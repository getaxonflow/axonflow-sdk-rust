//! A compiled binary that constructs a client, makes ONE call, and returns from
//! `main` — the shape of a CLI, a Lambda handler, or a CI step.
//!
//! This is the whole population the first-request heartbeat trigger exists to
//! make visible, and it is the shape a SPAWNED ping loses: the process exits,
//! the tokio runtime is dropped, and the in-flight POST is cancelled.
//!
//! Measured before the fix: 1 delivery in 12 runs — and worse than silence, the
//! `/health` GET reached the platform every time, so the SDK made an
//! unsolicited request to someone else's server and recorded nothing for it.
//!
//! No sleep, no join, no `flush`. Adding any of those would make this fixture
//! unable to express the defect, which is exactly how a fixture comes to read
//! as a disproof of a bug it never gave itself a chance to see.

use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};

#[tokio::main]
async fn main() {
    let endpoint = std::env::args().nth(1).expect("usage: <endpoint>");
    let client = AxonFlowClient::new(AxonFlowConfig::new(&endpoint)).expect("client");
    // One call. Its outcome is irrelevant — the heartbeat rides the ATTEMPT.
    let _ = client.list_connectors().await;
}
