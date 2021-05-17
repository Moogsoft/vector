package metadata

components: sinks: moogsoft: {
	title: "Moogsoft"

	classes: {
		commonly_used: false
		delivery:      "at_least_once"
		development:   "stable"
		egress_method: "batch"
		service_providers: ["Moogsoft"]
		stateful: false
	}

	features: {
		buffer: enabled:      true
		healthcheck: enabled: false
		send: {
			batch: {
				enabled:      true
				common:       false
				max_bytes:    10485760
				timeout_secs: 1
			}
			compression: {
				enabled: true
				default: "none"
				algorithms: ["gzip"]
				levels: ["none", "fast", "default", "best", 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
			}
			encoding: {
				enabled: true
				codec: enabled: false
			}
			request: {
				enabled:                    true
				concurrency:                100
				rate_limit_duration_secs:   1
				rate_limit_num:             100
				retry_initial_backoff_secs: 1
				retry_max_duration_secs:    10
				timeout_secs:               30
				headers:                    false
			}
			tls: enabled: false
			to: {
				service: services.moogsoft

				interface: {
					socket: {
						api: {
							title: "Moogsoft"
							url:   urls.moogsoft_api
						}
						direction: "outgoing"
						protocols: ["http"]
						ssl: "required"
					}
				}
			}
		}
	}

	support: {
		targets: {
			"aarch64-unknown-linux-gnu":      true
			"aarch64-unknown-linux-musl":     true
			"armv7-unknown-linux-gnueabihf":  true
			"armv7-unknown-linux-musleabihf": true
			"x86_64-apple-darwin":            true
			"x86_64-pc-windows-msv":          true
			"x86_64-unknown-linux-gnu":       true
			"x86_64-unknown-linux-musl":      true
		}
		requirements: []
		warnings: []
		notices: []
	}

	configuration: {
		api_key: {
			description: "Your Moogsoft API key."
			required:    true
			warnings: []
			type: string: {
				examples: ["xxxx", "${MOOGSOFT_API_KEY}"]
				syntax: "literal"
			}
		}
		uri: {
			common:      true
			description: "Your Moogsoft API uri."
			required:    false
			warnings: []
			type: string: {
				default: null
				examples: ["http://api.moogsoft.ai/v1/integrations/metrics"]
				syntax: "literal"
			}
		}
	}

	input: {
		logs:    true
		metrics: null
	}

	telemetry: metrics: {
		events_discarded_total: components.sources.internal_metrics.output.metrics.events_discarded_total
		processed_bytes_total:  components.sources.internal_metrics.output.metrics.processed_bytes_total
	}
}
