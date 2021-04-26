#!/usr/bin/env bash
set -euo pipefail

# release-s3.sh
#
# SUMMARY
#
#   Uploads archives and packages to S3

CHANNEL="${CHANNEL:-"$(scripts/release-channel.sh)"}"
VERSION="${VERSION:-"$(scripts/version.sh)"}"
DATE="${DATE:-"$(date -u +%Y-%m-%d)"}"

#
# Setup
#

td="$(mktemp -d)"
cp -av "target/artifacts/." "$td"
ls "$td"

td_nightly="$(mktemp -d)"
cp -av "target/artifacts/." "$td_nightly"

for f in "$td_nightly"/*; do
    a="$(echo "$f" | sed -r -e "s/$VERSION/nightly/")"
    mv "$f" "$a"
done
ls "$td_nightly"

td_latest="$(mktemp -d)"
cp -av "target/artifacts/." "$td_latest"

for f in "$td_latest"/*; do
    a="$(echo "$f" | sed -r -e "s/$VERSION/latest/")"
    mv "$f" "$a"
done
ls "$td_latest"


#
# Upload
#

if [[ "$CHANNEL" == "nightly" ]]; then
  # Add nightly files with the $DATE for posterity
  echo "Uploading all artifacts to s3://collector-tar/vector/nightly/$DATE"
  aws s3 cp "$td_nightly" "s3://collector-tar/vector/nightly/$DATE" --recursive --sse --acl private
  echo "Uploaded archives"

  # Add "latest" nightly files
  echo "Uploading all artifacts to s3://collector-tar/vector/nightly/latest"
  aws s3 rm --recursive "s3://collector-tar/vector/nightly/latest"
  aws s3 cp "$td_nightly" "s3://collector-tar/vector/nightly/latest" --recursive --sse --acl private
  echo "Uploaded archives"

elif [[ "$CHANNEL" == "latest" ]]; then
  VERSION_EXACT="$VERSION"
  # shellcheck disable=SC2001
  VERSION_MINOR_X="$(echo "$VERSION" | sed 's/\.[0-9]*$/.X/g')"
  # shellcheck disable=SC2001
  VERSION_MAJOR_X="$(echo "$VERSION" | sed 's/\.[0-9]*\.[0-9]*$/.X/g')"

  for i in "$VERSION_EXACT" "$VERSION_MINOR_X" "$VERSION_MAJOR_X" "latest"; do
    # Upload the specific version
    echo "Uploading artifacts to s3://collector-tar/vector/$i/"
    aws s3 cp "$td" "s3://collector-tar/vector/$i/" --recursive --sse --acl private

    # Delete anything that isn't the current version
    echo "Deleting old artifacts from s3://collector-tar/vector/$i/"
    aws s3 rm "s3://collector-tar/vector/$i/" --recursive --exclude "*$VERSION_EXACT*"
    echo "Deleted old versioned artifacts"
  done
fi

#
# Cleanup
#

rm -rf "$td"
rm -rf "$td_nightly"
rm -rf "$td_latest"
