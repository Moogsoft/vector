#!/usr/bin/env bash
set -euo pipefail

# release-s3.sh
#
# SUMMARY
#
#   Uploads archives and packages to S3

#CHANNEL="${CHANNEL:-"$(scripts/release-channel.sh)"}"
CHANNEL="latest"
VERSION="${VECTOR_VERSION:-"$(scripts/version.sh)"}"
DATE="${DATE:-"$(date -u +%Y-%m-%d)"}"

export AWS_REGION=us-east-1

#
# Setup
#

echo "Starting S3 release"

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

echo "Unpacking plugins"
td_plugins="$(mktemp -d)"
tar -xzf "target/plugins/collector-$VERSION-plugins.tar.gz" -C "$td_plugins/"

ls "$td_plugins"

echo "Moving plugins to artifact destinations"
cp -R "$td_plugins/plugins" "$td/"
cp -R "$td_plugins/plugins" "$td_nightly/"
cp -R "$td_plugins/plugins" "$td_latest/"

#
# A helper function for verifying a published artifact.
#
verify_artifact() {
  local URL="$1"
  local FILENAME="$2"
  echo "Verifying $URL"
  cmp <(wget -qO- --retry-on-http-error=404 --wait 10 --tries "$VERIFY_RETRIES" "$URL") "$FILENAME"
}

#
# Upload
#

if [[ "$CHANNEL" == "nightly" ]]; then
  # Add nightly files with the $DATE for posterity
  echo "Uploading all artifacts to s3://${S3_BUCKET}/vector/nightly/$DATE"
  aws s3 --region "${BUCKET_REGION}" cp "$td_nightly" "s3://${S3_BUCKET}/vector/nightly/$DATE" --recursive --sse --acl private
  echo "Uploaded archives"

  # Add "latest" nightly files
  echo "Uploading all artifacts to s3://${S3_BUCKET}/vector/nightly/latest"
  aws s3 --region "${BUCKET_REGION}" rm --recursive "s3://${S3_BUCKET}/vector/nightly/latest"
  aws s3 --region "${BUCKET_REGION}" cp "$td_nightly" "s3://${S3_BUCKET}/vector/nightly/latest" --recursive --sse --acl private
  echo "Uploaded archives"

elif [[ "$CHANNEL" == "latest" ]]; then
  VERSION_EXACT="$VERSION"
  # shellcheck disable=SC2001
  VERSION_MINOR_X="$(echo "$VERSION" | sed 's/\.[0-9]*$/.X/g')"
  # shellcheck disable=SC2001
  VERSION_MAJOR_X="$(echo "$VERSION" | sed 's/\.[0-9]*\.[0-9]*$/.X/g')"

  for i in "$VERSION_EXACT" "$VERSION_MINOR_X" "$VERSION_MAJOR_X" "latest"; do
    if [[ -z "$PLUGIN_NAMESPACE" ]]; then
      # Upload the specific version
      echo "Uploading artifacts to s3://${S3_BUCKET}/vector/$i/"
      aws s3 --region "${BUCKET_REGION}" cp "$td" "s3://${S3_BUCKET}/vector/$i/" --recursive --sse --acl private

      # Delete anything that isn't the current version
      echo "Deleting old artifacts from s3://${S3_BUCKET}/vector/$i/"
      aws s3 --region "${BUCKET_REGION}" rm "s3://${S3_BUCKET}/vector/$i/" --recursive --exclude "*$VERSION_EXACT*" --exclude "*plugins*"
      echo "Deleted old versioned artifacts"

      # Delete any deprecated plugins MCP-1627
      echo "Deleting deprecated plugins from s3://${S3_BUCKET}/vector/$i/"
      if aws s3 --region "${BUCKET_REGION}" ls "s3://${S3_BUCKET}/vector/$i/plugins/" >> /dev/null 2>&1; then
        for j in $(aws s3 --region "${BUCKET_REGION}" ls "s3://${S3_BUCKET}/vector/$i/plugins/" | awk '{print $2}' | sed 's/\///'); do
          if [ ! -d "$td/plugins/$j" ]; then
            echo "Deleting folder ${S3_BUCKET}/vector/$i/plugins/$j , this folder does not exist in the current build and we can assume this plugin is deprecated."
            aws s3 --region "${BUCKET_REGION}" rm "s3://${S3_BUCKET}/vector/$i/plugins/$j" --recursive
          fi
        done
      else
        echo "No plugins directory detected in S3, not removing any deprecated plugins"
      fi
    else
      echo "Uploading artifacts to s3://${S3_BUCKET}/vector/namespaces/$PLUGIN_NAMESPACE/$i/"
      aws s3 --region "${BUCKET_REGION}" cp "$td" "s3://${S3_BUCKET}/vector/namespaces/$PLUGIN_NAMESPACE/$i/" --recursive --sse --acl private

      # Delete anything that isn't the current version
      echo "Deleting old artifacts from s3://${S3_BUCKET}/vector/namespaces/$PLUGIN_NAMESPACE/$i/"
      aws s3 --region "${BUCKET_REGION}" rm "s3://${S3_BUCKET}/vector/namespaces/$PLUGIN_NAMESPACE/$i/" --recursive --exclude "*$VERSION_EXACT*" --exclude "*plugins*"
      echo "Deleted old versioned artifacts"

      # Delete any deprecated plugins MCP-1627
      echo "Deleting deprecated plugins from s3://${S3_BUCKET}/vector/namespaces/$PLUGIN_NAMESPACE/$i/"
      if aws s3 --region "${BUCKET_REGION}" ls "s3://${S3_BUCKET}/vector/namespaces/$PLUGIN_NAMESPACE/$i/plugins/" >> /dev/null 2>&1; then
        for j in $(aws s3 --region "${BUCKET_REGION}" ls "s3://${S3_BUCKET}/vector/namespaces/$PLUGIN_NAMESPACE/$i/plugins/" | awk '{print $2}' | sed 's/\///'); do
          if [ ! -d "$td/plugins/$j" ]; then
            echo "Deleting folder ${S3_BUCKET}/vector/namespaces/$PLUGIN_NAMESPACE/$i/plugins/$j , this folder does not exist in the current build and we can assume this plugin is deprecated."
            aws s3 --region "${BUCKET_REGION}" rm "s3://${S3_BUCKET}/vector/namespaces/$PLUGIN_NAMESPACE/$i/plugins/$j" --recursive
          fi
        done
      else
        echo "No plugins directory detected in S3, not removing any deprecated plugins"
      fi
    fi
  done
fi

#
# Cleanup
#

rm -rf "$td"
rm -rf "$td_nightly"
