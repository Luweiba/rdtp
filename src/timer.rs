use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::{oneshot, Mutex};
use tokio::time::Duration;

/// 用异步方式定时器
pub struct Timer {
    interval: u64,
    expected_count: Arc<Mutex<u32>>,
    inner_count: Arc<Mutex<u32>>,
    rto_sender: Sender<()>,
}

impl Timer {
    pub fn new(interval: u64, rto_sender: Sender<()>) -> Self {
        let mut inner_count = Arc::new(Mutex::new(0u32));
        let expected_count = Arc::new(Mutex::new(0u32));
        Self {
            interval,
            expected_count,
            inner_count,
            rto_sender,
        }
    }

    pub async fn flush_timing(&mut self) {
        // 计时 + 1
        let expected_count_clone = self.expected_count.clone();
        {
            let mut expected_count_lock = expected_count_clone.lock().await;
            *expected_count_lock += 1;
        }
        // 释放expected_count_lock锁
        let (sender, receiver) = oneshot::channel::<()>();
        let interval = self.interval.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(interval)).await;
            sender.send(());
        });
        let rto_sender_clone = self.rto_sender.clone();
        let expected_count_clone = self.expected_count.clone();
        let inner_count_clone = self.inner_count.clone();
        tokio::spawn(async move {
            receiver.await;
            // 获取inner_count_lock
            let inner_count_snapshot;
            {
                let mut inner_count_lock = inner_count_clone.lock().await;
                *inner_count_lock += 1;
                inner_count_snapshot = *inner_count_lock;
            }
            {
                let expected_count_lock = expected_count_clone.lock().await;
                if inner_count_snapshot == *expected_count_lock {
                    rto_sender_clone.send(()).await;
                }
            }
        });
    }
}
