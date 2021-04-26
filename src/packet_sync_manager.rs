use crate::timer::Timer;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio::time::{Duration, Instant};

pub struct SyncManager {
    // Sync  Manager ID
    id: u16,
    // rto 超时时间，以毫秒（ms）计数
    rto: u32,
}

impl SyncManager {
    pub fn new(id: u16, rto: u32) -> Self {
        Self { id, rto }
    }

    /// 开始跟踪该包的同步
    /// `@param` packet_info 二元组，标识数据包的起始序号与终止序号
    /// `@param` ack_receiver 接收ack与nak信号
    /// `@param` rt_sender 检测到错误或超时事件则要求Packet Sender重传
    /// `@param` sync_sender 同步结束后递增同步包
    pub async fn start_send_tracking(
        self,
        packet_info: (usize, usize),
        mut ack_receiver: Receiver<bool>,
        rt_sender: Sender<(u16, usize, usize)>,
        sync_sender: Sender<(u16, usize, usize)>,
    ) -> Result<(), Box<dyn Error>> {
        log::info!(
            "Sync Manager {} Begin tracking {}-{}",
            self.id,
            packet_info.0,
            packet_info.1
        );
        let (ack_or_nak_sender, mut ack_or_nak_receiver) = tokio::sync::mpsc::channel::<bool>(3);
        let rt_sender_clone = rt_sender.clone();
        let sync_id = self.id;
        tokio::spawn(async move {
            loop {
                if let Some(is_ack) = ack_receiver.recv().await {
                    if is_ack {
                        // 收到确认信号
                        ack_or_nak_sender.send(true).await;
                        // 此包对面已经收到
                        break;
                    } else {
                        // 告知信息，刷新超时计时器
                        ack_or_nak_sender.send(false).await;
                        rt_sender_clone.send((sync_id, packet_info.0, packet_info.1));
                    }
                }
            }
        });
        let (rto_sender, mut rto_receiver) = tokio::sync::mpsc::channel::<()>(3);
        let mut timer = Timer::new(self.rto as u64, rto_sender);
        // 开始计时
        timer.flush_timing().await;
        let timer = Arc::new(Mutex::new(timer));
        loop {
            tokio::select! {
                ack_or_nak = ack_or_nak_receiver.recv() => {
                    if let Some(is_ack) = ack_or_nak {
                        if is_ack {
                            break;
                        } else {
                            // 包错误，重传
                            rt_sender.send((sync_id, packet_info.0, packet_info.1)).await;
                            {
                                let mut timer_lock = timer.lock().await;
                                // 重启计时
                                timer_lock.flush_timing().await;
                            }
                        }
                    }
                },
                // 超时
                _ = rto_receiver.recv() => {
                    log::info!("{:?}: Sync Manager {} 检测到超时 {}-{} ", Instant::now(), sync_id, packet_info.0, packet_info.1);
                    rt_sender.send((sync_id, packet_info.0, packet_info.1)).await;
                    {
                        let mut timer_lock = timer.lock().await;
                        // 重启计时
                        timer_lock.flush_timing().await;
                    }
                }
            }
        }
        // 等3 * self.rto
        sleep(Duration::from_millis((3 * self.rto) as u64)).await;
        // 发送同步成功信息
        sync_sender
            .send((sync_id, packet_info.0, packet_info.1))
            .await;
        Ok(())
    }

    /// 开始跟踪该包的同步
    /// `@param` packet_info 二元组，标识数据包的起始序号与终止序号
    /// `@param` ack_receiver 接收ack与nak信号
    /// `@param` rt_sender 检测到错误或超时事件则要求Packet Sender重传
    /// `@param` sync_sender 同步结束后递增同步包
    pub async fn start_recv_tracking(
        self,
        packet_info: (usize, usize),
        mut duplicated_receiver: Receiver<()>,
        rt_ack_sender: Sender<(u16, u32, u32, u32, u32)>,
        sync_sender: Sender<(u16, usize, usize)>,
    ) -> Result<(), Box<dyn Error>> {
        log::info!(
            "Sync Manager {} Begin tracking {}-{}",
            self.id,
            packet_info.0,
            packet_info.1
        );
        let (inner_sync_sender, mut inner_sync_receiver) = tokio::sync::mpsc::channel::<()>(3);
        // 等3 * self.rto
        let mut timer = Timer::new(3 * self.rto as u64, inner_sync_sender);
        timer.flush_timing().await;
        let sync_id = self.id;
        loop {
            tokio::select! {
                msg = duplicated_receiver.recv() => {
                    log::info!("Sync Manager {} 检测到重复包 {}-{}", sync_id, packet_info.0, packet_info.1);
                    if msg.is_some() {
                        // 收到重复包，意味着对面没有收到ACK
                        // 简单重置定时器即可
                        // rt_ack_sender.send((self.id, packet_info.0 as u32, packet_info.1 as u32, 0, self.rto)).await;
                        timer.flush_timing().await;
                    }
                },
                _ = inner_sync_receiver.recv() => {
                    break;
                }
            }
        }
        // 发送同步成功信息
        sync_sender
            .send((self.id, packet_info.0, packet_info.1))
            .await;
        Ok(())
    }
}
