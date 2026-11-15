#!/bin/sh
# Removes everything setup.sh created (same NAME / REGION): API, function, logs, image
# repository, model bucket and IAM role. Asks before deleting.
#
#   NAME=poodpim-asr REGION=ap-southeast-7 ./deploy/lambda/teardown.sh
set -u

REGION=${REGION:-ap-southeast-7}
NAME=${NAME:-poodpim-asr}
ACCT=$(aws sts get-caller-identity --query Account --output text) || exit 1
BUCKET=${BUCKET:-$NAME-models-$ACCT-$REGION}
ROLE=$NAME-lambda-role

echo "This deletes, in $REGION: API + Lambda function '$NAME', its logs, ECR repo '$NAME',"
echo "S3 bucket '$BUCKET' (with the model) and IAM role '$ROLE'."
if [ "${YES:-}" != 1 ]; then
  printf "Type the name (%s) to confirm: " "$NAME"
  read -r answer
  [ "$answer" = "$NAME" ] || { echo "cancelled"; exit 1; }
fi

API_ID=$(aws apigatewayv2 get-apis --region "$REGION" --query "Items[?Name=='$NAME'].ApiId | [0]" --output text)
[ -n "$API_ID" ] && [ "$API_ID" != None ] && aws apigatewayv2 delete-api --region "$REGION" --api-id "$API_ID" && echo "API deleted"
aws lambda delete-function --region "$REGION" --function-name "$NAME" && echo "function deleted"
aws logs delete-log-group --region "$REGION" --log-group-name "/aws/lambda/$NAME" && echo "logs deleted"
aws ecr delete-repository --region "$REGION" --repository-name "$NAME" --force >/dev/null && echo "ECR repo deleted"
aws s3 rb "s3://$BUCKET" --force --region "$REGION" >/dev/null && echo "bucket deleted"
aws iam delete-role-policy --role-name "$ROLE" --policy-name model-read
aws iam detach-role-policy --role-name "$ROLE" --policy-arn arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole
aws iam delete-role --role-name "$ROLE" && echo "role deleted"
