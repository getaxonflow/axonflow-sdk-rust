# Hosted 402 and monthly-cap 429 runtime proof

Run each case against the disposable Hosted staging Agent. The helper accepts only `https://try-staging.getaxonflow.com`, makes one harmless synthetic governed request, and prints no credential.

The Portal issues linked credentials only after payment. For the `402` proof, use a staging tenant that was paid, linked, and then canceled so its subscription is no longer active, or a controlled staging fixture with an equivalent inactive subscription. Do not assume a newly registered unpaid Basic tenant can obtain a linked credential. For the `429` proof, use a paid staging tenant whose monthly governed-event allowance has been exhausted by a controlled fixture. Use separate credentials for the two cases.

```bash
AXONFLOW_HOSTED_STAGING_AGENT_URL=https://try-staging.getaxonflow.com \
AXONFLOW_HOSTED_STAGING_CLIENT_ID='<inactive-subscription-tenant>' \
AXONFLOW_HOSTED_STAGING_CLIENT_SECRET='<staging-secret>' \
AXONFLOW_HOSTED_EXPECTED_STATUS=402 \
./runtime-e2e/hosted_paid_refusal/test.sh

AXONFLOW_HOSTED_STAGING_AGENT_URL=https://try-staging.getaxonflow.com \
AXONFLOW_HOSTED_STAGING_CLIENT_ID='<monthly-capped-tenant>' \
AXONFLOW_HOSTED_STAGING_CLIENT_SECRET='<staging-secret>' \
AXONFLOW_HOSTED_EXPECTED_STATUS=429 \
./runtime-e2e/hosted_paid_refusal/test.sh
```

Both real Agent runs are release gates. Local HTTP contract tests prove parsing and fail-closed behavior but do not replace live checks.
