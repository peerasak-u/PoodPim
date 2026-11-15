#!/bin/sh
# Rebuild the Lambda image and roll it out to the existing function.
# One-time infrastructure (ECR repo, S3 model bucket, IAM role, HTTP API) is described in README.md.
set -eu
cd "$(dirname "$0")/../.."

REGION=${REGION:-ap-southeast-7}
FUNCTION=${FUNCTION:-thocr-asr}
ACCT=$(aws sts get-caller-identity --query Account --output text)
REG=$ACCT.dkr.ecr.$REGION.amazonaws.com
TAG=v$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)-$(date +%Y%m%d%H%M)

docker buildx build --platform linux/arm64 --provenance=false --load -f deploy/lambda/Dockerfile -t thocr-asr:dev .
aws ecr get-login-password --region "$REGION" | docker login --username AWS --password-stdin "$REG" >/dev/null
docker tag thocr-asr:dev "$REG/thocr-asr:$TAG"
docker push "$REG/thocr-asr:$TAG"
aws lambda update-function-code --region "$REGION" --function-name "$FUNCTION" --image-uri "$REG/thocr-asr:$TAG" >/dev/null
aws lambda wait function-updated-v2 --region "$REGION" --function-name "$FUNCTION"
echo "deployed thocr-asr:$TAG to $FUNCTION ($REGION)"
