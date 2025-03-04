use std::{collections::BinaryHeap, fmt};

use buffers::{Ackable, Acker};
use futures::{stream::FuturesUnordered, FutureExt, Stream, StreamExt, TryFutureExt};
use tokio::{pin, select, task::JoinError};
use tower::{Service, ServiceExt};
use tracing::Instrument;

use crate::event::{EventStatus, Finalizable};

#[derive(Eq)]
struct PendingAcknowledgement {
    seq_no: u64,
    ack_size: usize,
}

impl PartialEq for PendingAcknowledgement {
    fn eq(&self, other: &Self) -> bool {
        self.seq_no == other.seq_no
    }
}

impl PartialOrd for PendingAcknowledgement {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // Reverse ordering so that in a `BinaryHeap`, the lowest sequence number is the highest priority.
        Some(other.seq_no.cmp(&self.seq_no))
    }
}

impl Ord for PendingAcknowledgement {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .partial_cmp(self)
            .expect("PendingAcknowledgement should always return a valid comparison")
    }
}

/// Drives the interaction between a stream of items and a service which processes them
/// asynchronously.
///
/// `Driver`, as a high-level, facilitates taking items from an arbitrary `Stream` and pushing them
/// through a `Service`, spawning each call to the service so that work can be run concurrently,
/// managing waiting for the service to be ready before processing more items, and so on.
///
/// Additionally, `Driver` handles two event-specific facilities: finalization and acknowledgement.
///
/// This capability is parameterized so any implementation which can define how to interpret the
/// response for each request, as well as define how many events a request is compromised of, can be
/// used with `Driver`.
pub struct Driver<St, Svc> {
    input: St,
    service: Svc,
    acker: Acker,
}

impl<St, Svc> Driver<St, Svc> {
    pub fn new(input: St, service: Svc, acker: Acker) -> Self {
        Self {
            input,
            service,
            acker,
        }
    }
}

impl<St, Svc> Driver<St, Svc>
where
    St: Stream,
    St::Item: Ackable + Finalizable,
    Svc: Service<St::Item>,
    Svc::Error: fmt::Debug + 'static,
    Svc::Future: Send + 'static,
    Svc::Response: AsRef<EventStatus>,
{
    /// Runs the driver until the input stream is exhausted.
    ///
    /// All in-flight calls to the provided `service` will also be completed before `run` returns.
    ///
    /// # Errors
    ///
    /// No errors are currently returned.  The return type is purely to simplify caller code, but may
    /// return an error for a legitimate reason in the future.
    pub async fn run(self) -> Result<(), ()> {
        let mut in_flight = FuturesUnordered::new();
        let mut pending_acks = BinaryHeap::new();
        let mut seq_head: u64 = 0;
        let mut seq_tail: u64 = 0;

        let Self {
            input,
            mut service,
            acker,
        } = self;

        pin!(input);

        loop {
            select! {
                // Using `biased` ensures we check the branches in the order they're written, and
                // the way they're ordered is to ensure that we're reacting to completed requests as
                // soon as possible to acknowledge them and make room for more requests to be processed.
                biased;

                // One of our service calls has completed.
                Some(result) = in_flight.next() => {
                    // Rebind so we can annotate the type, otherwise the compiler is inexplicably angry.
                    let result: Result<(u64, usize), JoinError> = result;
                    match result {
                        Ok((seq_no, ack_size)) => {
                            trace!(message = "Sending request.", seq_no, ack_size);
                            pending_acks.push(PendingAcknowledgement { seq_no, ack_size });

                            let mut num_to_ack = 0;
                            while let Some(pending_ack) = pending_acks.peek() {
                                if pending_ack.seq_no == seq_tail {
                                    let PendingAcknowledgement { ack_size, .. } = pending_acks.pop()
                                        .expect("should not be here unless pending_acks is non-empty");
                                    num_to_ack += ack_size;
                                    seq_tail += 1;
                                } else {
                                    break
                                }
                            }

                            if num_to_ack > 0 {
                                trace!(message = "Acking events.", ack_size = num_to_ack);
                                acker.ack(num_to_ack);
                            }
                        },
                        Err(e) => {
                            if e.is_panic() {
                                error!("driver service call unexpectedly panicked");
                            }

                            if e.is_cancelled() {
                                error!("driver service call unexpectedly cancelled");
                            }
                        },
                    }
                }

                // We've received an item from the input stream.
                Some(req) = input.next() => {
                    // Rebind the variable to avoid a bug with the pattern matching
                    // in `select!`: https://github.com/tokio-rs/tokio/issues/4076
                    let mut req = req;
                    let seqno = seq_head;
                    seq_head += 1;

                    trace!(
                        message = "Submitting service request.",
                        in_flight_requests = in_flight.len()
                    );
                    let ack_size = req.ack_size();
                    let finalizers = req.take_finalizers();

                    let svc = service.ready().await.expect("should not get error when waiting for svc readiness");
                    let fut = svc.call(req)
                        .err_into()
                        .map(move |result: Result<Svc::Response, Svc::Error>| {
                            let status = match result {
                                Err(error) => {
                                    error!(message = "Service call failed.", ?error, seqno);
                                    EventStatus::Failed
                                },
                                Ok(response) => {
                                    trace!(message = "Service call succeeded.", seqno);
                                    *response.as_ref()
                                }
                            };
                            finalizers.update_status(status);

                            (seqno, ack_size)
                        })
                        .instrument(info_span!("request", request_id = %seqno));

                    let handle = tokio::spawn(fut);
                    in_flight.push(handle);
                }

                else => {
                    break
                }
            }
        }

        Ok(())
    }
}
