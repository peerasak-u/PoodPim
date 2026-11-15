#!/bin/sh
# Rebuild the Lambda image and roll it out to an existing function made by setup.sh.
#   NAME=poodpim-asr REGION=ap-southeast-7 ./deploy/lambda/deploy.sh
# (The original deployment is named thocr-asr: NAME=thocr-asr ./deploy/lambda/deploy.sh)
set -eu
cd "$(dirname "$0")/../.."

REGION=${REGION:-ap-southeast-7}
NAME=${NAME:-${FUNCTION:-poodpim-asr}}
ACCT=$(aws sts get-caller-identity --query Account --output text)
REG=$ACCT.dkr.ecr.$REGION.amazonaws.com
TAG=v$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)-$(date +%Y%m%d%H%M%S)

docker buildx build --platform linux/arm64 --provenance=false --load -f deploy/lambda/Dockerfile -t "$NAME:build" .
# Fetch first: in a pipe, sh would hide an aws failure behind a vague docker error.
ECR_PASS=$(aws ecr get-login-password --region "$REGION")
printf '%s' "$ECR_PASS" | docker login --username AWS --password-stdin "$REG" >/dev/null
docker tag "$NAME:build" "$REG/$NAME:$TAG"
docker push --quiet "$REG/$NAME:$TAG" >/dev/null
aws lambda update-function-code --region "$REGION" --function-name "$NAME" --image-uri "$REG/$NAME:$TAG" >/dev/null
aws lambda wait function-updated-v2 --region "$REGION" --function-name "$NAME"
echo "deployed $NAME:$TAG ($REGION)"
