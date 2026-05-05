use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use extensions::host::event_bus::{EventBus, spawn_listener};

// ============================================================================
// Tests: Basic emit and receive
// ============================================================================

#[tokio::test]
async fn test_emit_and_receive() {
    let bus = EventBus::new(16);
    let mut rx = bus.subscribe();
    bus.emit(42);

    let result = tokio::time::timeout(Duration::from_secs(1), async {
        rx.recv().await.unwrap()
    })
    .await
    .unwrap();
    assert_eq!(result, 42);
}

// ============================================================================
// Tests: Multiple subscribers receive the same event
// ============================================================================

#[tokio::test]
async fn test_multiple_subscribers_receive() {
    let bus = EventBus::new(16);
    let mut rx1 = bus.subscribe();
    let mut rx2 = bus.subscribe();

    bus.emit("hello".to_string());

    let msg1 = tokio::time::timeout(Duration::from_secs(1), rx1.recv())
        .await
        .unwrap()
        .unwrap();
    let msg2 = tokio::time::timeout(Duration::from_secs(1), rx2.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(msg1, "hello");
    assert_eq!(msg2, "hello");
}

// ============================================================================
// Tests: Lagged listener
// ============================================================================

#[tokio::test]
async fn test_lagged_listener() {
    // Use a very small capacity to force lag
    let bus = EventBus::new(2);
    let mut rx = bus.subscribe();

    // Emit 3 events while no one is consuming
    // With capacity=2, the oldest event will be evicted
    bus.emit(1);
    bus.emit(2);
    bus.emit(3);

    // Now try to receive — the first recv should get Lagged
    // because event 1 was evicted
    let result = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .unwrap();

    match result {
        Ok(value) => {
            // If we receive a value, it might be 2 or 3 (1 was evicted)
            assert!([2, 3].contains(&value), "should receive 2 or 3, got {}", value);
        }
        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
            // This is what we expect — the listener lagged
            assert!(n >= 1, "should lag by at least 1 message");
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
            panic!("channel should not be closed");
        }
    }
}

// ============================================================================
// Tests: Handler timeout drops slow observational handlers
// ============================================================================

#[tokio::test]
async fn test_handler_timeout_drops_slow() {
    let bus: EventBus<String> = EventBus::new(16);
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received.clone();

    // Spawn a listener with a slow handler (200ms > 100ms timeout)
    let _handle = spawn_listener(bus.subscribe(), move |msg: String| {
        let received = received_clone.clone();
        async move {
            if msg == "slow" {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            received.lock().await.push(msg);
        }
    });

    bus.emit("fast".to_string());
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Fast message should be processed
    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0], "fast");
    drop(msgs);

    // Slow message should be silently dropped after 100ms timeout
    bus.emit("slow".to_string());
    tokio::time::sleep(Duration::from_millis(150)).await;

    // After timeout, the slow handler should NOT have completed
    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1, "slow handler should be dropped due to timeout");
}
