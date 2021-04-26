use crate::message_format::Message;
use crate::packet_sync_manager::SyncManager;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use tokio::net::{ToSocketAddrs, UdpSocket};
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::Instant;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use std::fs::File;
use std::io::{Write, BufWriter};

/// 一次发送的帧的大小
const FRAME_SIZE: usize = 1 << 10;
///
const SYNC_PACKET_MAX_SIZE: usize = 1 << 16;
/// 并发数量
const RECV_WND: u16 = 8;
/// 超时时延
const RTO: u32 = 50;
/// 丢包率
const LOSS_RATE: u32 = 10;

/// RDTPSocket 是可靠传输的编程套接字
/// 传输层协议为UDP
pub struct RDTPSocket {
    socket: Arc<Mutex<UdpSocket>>,
    recv_socket: Arc<Mutex<UdpSocket>>,
}

impl RDTPSocket {
    /// 绑定一个本地`IP+Port`二元组
    pub async fn bind<A: ToSocketAddrs>(addr: A, recv_addr: A) -> Result<Self, Box<dyn Error>> {
        let socket = UdpSocket::bind(addr).await?;
        let recv_socket = UdpSocket::bind(recv_addr).await?;
        Ok(Self {
            socket: Arc::new(Mutex::new(socket)),
            recv_socket: Arc::new(Mutex::new(recv_socket)),
        })
    }
    /// 连接一个远端`IP+Port`二元组
    pub async fn connect<A: ToSocketAddrs>(
        &mut self,
        remote_send_addr: A,
        remote_recv_addr: A,
    ) -> Result<(), Box<dyn Error>> {
        {
            let socket_lock = self.socket.lock().await;
            socket_lock.connect(remote_recv_addr).await?;
        }
        {
            let recv_socket_lock = self.recv_socket.lock().await;
            recv_socket_lock.connect(remote_send_addr).await?;
        }
        Ok(())
    }
    /// 发送数据
    pub async fn send(&mut self, buf: &[u8]) -> Result<usize, Box<dyn Error>> {
        let local_start_instant = Instant::now();
        let sync_packets_set = Arc::new(Mutex::new(Vec::<(Instant, u16)>::new()));
        let recv_wnd: u16 = RECV_WND;
        let bytes_length = buf.len();
        let packets_range = Arc::new(split_frame(bytes_length, FRAME_SIZE));
        let total_packets_num = packets_range.len();
        // 用于与特定包的同步追踪者通信
        let sync_manager_ack_sender_table =
            Arc::new(Mutex::new(HashMap::<u16, Sender<bool>>::new()));
        let semaphore_packet_send = Arc::new(Semaphore::new(recv_wnd as usize));
        // 开启一个协程用于接受ACK与NAK包
        // 配合sync_manager_table
        // 配合Rtt策略
        let (ack_stop_signal_sender, mut ack_stop_signal_receiver) =
            tokio::sync::mpsc::channel::<()>(3);
        let (rt_stop_signal_sender, mut rt_stop_signal_receiver) =
            tokio::sync::mpsc::channel::<()>(3);
        let (end_signal_sender, mut end_signal_receiver) = tokio::sync::mpsc::channel::<()>(3);
        let sync_manager_ack_sender_table_clone = sync_manager_ack_sender_table.clone();
        let recv_socket_clone = self.recv_socket.clone();
        tokio::spawn(async move {
            let recv_socket_lock = recv_socket_clone.lock().await;
            let mut recv_buf = vec![0u8; SYNC_PACKET_MAX_SIZE];
            loop {
                tokio::select! {
                    // 如果出现问题再修改
                    // 应该不会分片吧……
                    recv_bytes = recv_socket_lock.recv(&mut recv_buf[..]) => {
                        if let Ok(nbytes) = recv_bytes {
                            // 理论来讲ACK包和NAK包都是无数据负载的
                            assert_eq!(nbytes, 27);
                            if let Some(message) = Message::from_bytes(&recv_buf[..nbytes]) {
                                let (type_id, _recv_wnd, sync_number, seq, ack, _timestamp, _rto) = message.get_id_wnd_sync_seq_ack_timestamp_rto_field();
                                match type_id {
                                    // ACK
                                    1u8 => {
                                        log::info!("Recv ACK {}-{}", seq, ack);
                                        let sender;
                                        {
                                            let mut sync_manager_ack_sender_table_lock = sync_manager_ack_sender_table_clone.lock().await;
                                            sender = sync_manager_ack_sender_table_lock.remove(&sync_number)
                                        }
                                        if let Some(sender) = sender {
                                            sender.send(true).await;
                                        }
                                    },
                                    // 其他字段不处理
                                    _ => {}
                                }
                            }
                        }
                    },
                    _ = ack_stop_signal_receiver.recv() => {
                        log::info!("ACK Receiver Coroutine Stop Successfully");
                        break;
                    }
                }
            }
        });
        // 开启一个协程用于更新同步包
        // 收到更新后删除sync_manager
        let (sync_sender, mut sync_receiver) =
            tokio::sync::mpsc::channel::<(u16, usize, usize)>(100);
        // let sync_manager_ack_sender_table_clone = sync_manager_ack_sender_table.clone();
        let sync_packets_set_clone = sync_packets_set.clone();
        tokio::spawn(async move {
            let mut sync_num = 0;
            loop {
                if let Some(packet_info) = sync_receiver.recv().await {
                    let sync_id = packet_info.0;
                    {
                        let mut sync_packets_set_lock = sync_packets_set_clone.lock().await;
                        sync_packets_set_lock.push((Instant::now(), sync_id));
                    }
                    sync_num += 1;
                    log::info!(
                        "{:?}: Sync ID {} 已同步 [{}/{}]",
                        Instant::now(),
                        sync_id,
                        sync_num,
                        total_packets_num
                    );
                    if sync_num == total_packets_num {
                        log::info!("End Sending");
                        rt_stop_signal_sender.send(()).await;
                        ack_stop_signal_sender.send(()).await;
                        end_signal_sender.send(()).await;
                    }
                }
            }
        });
        // 发送包的逻辑
        // 使用条件变量，用于发送数据包、或者重传数据包
        // 发送数据包后创建sync_manager追踪同步过程
        let next_packet_index = Arc::new(Mutex::new(0));
        let next_sync_manager_id = Arc::new(Mutex::new(0));
        let (rt_sender, mut rt_receiver) = tokio::sync::mpsc::channel::<(u16, usize, usize)>(100);
        let buf = Arc::new(buf.to_vec());
        let buf_clone = buf.clone();
        let socket_clone = self.socket.clone();
        tokio::spawn(async move {
            let mut local_rng = StdRng::from_entropy();
            loop {
                tokio::select! {
                    // 重传
                    packet_info = rt_receiver.recv() => {
                        if let Some((sync_number, packet_begin, packet_end)) = packet_info {
                            log::info!("RT packet {}-{} from sync manager {}", packet_begin, packet_end, sync_number);
                            let packet_content = buf_clone[packet_begin..packet_end].to_vec();
                            let rto = RTO;
                            let timestamp = (Instant::now() - local_start_instant).as_millis() as u32;
                            let mut message = Message::new(0, total_packets_num as u16, recv_wnd, sync_number, packet_begin as u32, packet_end as u32, timestamp, rto, packet_content);
                            {
                                let t = local_rng.gen_range(0..1000) < LOSS_RATE;
                                if t {
                                    log::info!("RT Send a Broken Message {}-{}", packet_begin, packet_end);
                                    message.broken();
                                }
                            }
                            let send_bytes;
                            {
                                let socket_lock = socket_clone.lock().await;
                                send_bytes = socket_lock.send(&message.to_bytes()).await;
                            }
                            if let Ok(nbytes) = send_bytes {
                                log::info!("RT successfully");
                                assert_eq!(nbytes, packet_end - packet_begin + 27);
                            }
                        }
                    },
                    _ = rt_stop_signal_receiver.recv() => {
                        log::info!("Rt Coroutine Stop Successfully");
                        break;
                    }
                }
            }
        });

        for i in 0..recv_wnd {
            let socket_clone = self.socket.clone();
            let buf_clone = buf.clone();
            let packets_range_clone = packets_range.clone();
            let semaphore = semaphore_packet_send.clone();
            let next_sync_manager_id_clone = next_sync_manager_id.clone();
            let next_packet_index_clone = next_packet_index.clone();
            let rt_sender_clone = rt_sender.clone();
            let sync_sender_clone = sync_sender.clone();
            let sync_manager_ack_sender_table_clone = sync_manager_ack_sender_table.clone();
            tokio::spawn(async move {
                let mut local_rng = StdRng::from_entropy();
                loop {
                    if let Ok(_permit) = semaphore.acquire().await {
                        let next_packet_index;
                        {
                            let mut next_packet_index_lock = next_packet_index_clone.lock().await;
                            next_packet_index = *next_packet_index_lock;
                            *next_packet_index_lock += 1;
                        }
                        let next_sync_manager_id;
                        {
                            let mut next_sync_manager_id_lock =
                                next_sync_manager_id_clone.lock().await;
                            next_sync_manager_id = *next_sync_manager_id_lock;
                            *next_sync_manager_id_lock += 1;
                        }
                        if next_packet_index >= total_packets_num {
                            log::info!("[{}] Stopped", i);
                            break;
                        }
                        let packet_range = packets_range_clone[next_packet_index];
                        let packet_content = buf_clone[packet_range.0..packet_range.1].to_vec();
                        let rto = RTO;
                        let timestamp = (Instant::now() - local_start_instant).as_millis() as u32;
                        let mut message = Message::new(
                            0,
                            total_packets_num as u16,
                            recv_wnd,
                            next_sync_manager_id,
                            packet_range.0 as u32,
                            packet_range.1 as u32,
                            timestamp,
                            rto,
                            packet_content,
                        );
                        log::info!("[{}] Send message {}-{}", i, packet_range.0, packet_range.1);
                        {
                            let t = local_rng.gen_range(0..1000) < LOSS_RATE;
                            if t {
                                log::info!("Send a Broken Message {}-{}", packet_range.0, packet_range.1);
                                message.broken();
                            }
                        }
                        let send_bytes;
                        {
                            let socket_lock = socket_clone.lock().await;
                            send_bytes = socket_lock.send(&message.to_bytes()).await;
                        }
                        if let Ok(nbytes) = send_bytes {
                            assert_eq!(nbytes, packet_range.1 - packet_range.0 + 27);
                            // 创建SyncManager用于追踪同步
                            let sync_manager = SyncManager::new(next_sync_manager_id, rto);
                            let (ack_sender, ack_receiver) = tokio::sync::mpsc::channel::<bool>(3);
                            let sync_future = sync_manager.start_send_tracking(
                                packet_range,
                                ack_receiver,
                                rt_sender_clone.clone(),
                                sync_sender_clone.clone(),
                            );
                            {
                                let mut sync_manager_ack_sender_table_lock =
                                    sync_manager_ack_sender_table_clone.lock().await;
                                sync_manager_ack_sender_table_lock
                                    .insert(next_sync_manager_id, ack_sender);
                            }
                            // 等待同步结束
                            sync_future.await;
                            // 终止发包
                            {
                                let next_packet_index_lock =
                                    next_packet_index_clone.lock().await;
                                if *next_packet_index_lock == total_packets_num {
                                    log::info!("[{}] Stopped", i);
                                    break;
                                }
                            }
                        }
                    } else {
                        log::warn!("Semaphore Acquire Error");
                        continue;
                    }
                }
            });
        }
        end_signal_receiver.recv().await;
        log::info!("Final End");
        let mut sync_packets_set_lock = sync_packets_set.lock().await;
        sync_packets_set_lock.sort_by(|a, b| a.1.cmp(&b.1));
        let mut send_file = File::create("./send_sync_data.txt")?;
        let mut file_writer = BufWriter::new(send_file);
        for pair in sync_packets_set_lock.iter() {
            file_writer.write(format!("{}:{:?}\n", pair.1, pair.0).as_bytes());
        }
        Ok(total_packets_num)
    }
    /// 接收数据
    pub async fn recv(&self, buf: &mut [u8]) -> Result<usize, Box<dyn Error>> {
        let sync_packets_set = Arc::new(Mutex::new(Vec::<(Instant, u16)>::new()));
        let recv_wnd: u16 = RECV_WND;
        // 用于与特定包的同步追踪者通信
        let sync_manager_ack_sender_table =
            Arc::new(Mutex::new(HashMap::<u16, Sender<bool>>::new()));
        let mut duplicated_packet_table = HashMap::<u16, Sender<()>>::new();
        let mut payloads = Vec::new();
        let mut total_packets_num = 0;
        // 开启线程用于听取发送ACK消息
        let (ack_sender, mut ack_receiver) =
            tokio::sync::mpsc::channel::<(u16, u32, u32, u32, u32)>(100);
        let socket_clone = self.socket.clone();
        let (ack_stop_signal_sender, mut ack_stop_signal_receiver) = tokio::sync::mpsc::channel::<()>(3);
        let (recv_stop_signal_sender, mut recv_stop_signal_receiver) = tokio::sync::mpsc::channel::<()>(3);
        tokio::spawn(async move {
            let socket_lock = socket_clone.lock().await;
            let mut local_rng = StdRng::from_entropy();
            let mut broken_flag = false;
            loop {
                tokio::select! {
                    ack_msg = ack_receiver.recv() => {
                        if let Some((sync_number, seq, ack, timestamp, rto)) = ack_msg {
                            let mut ack_message = Message::new(
                                1u8,
                                0,
                                recv_wnd,
                                sync_number,
                                seq as u32,
                                ack as u32,
                                timestamp,
                                rto,
                                vec![],
                            );
                            {
                                let t = local_rng.gen_range(0..1000) < LOSS_RATE;
                                if t {
                                    log::info!("Send a Broken Message {}-{}", seq, ack);
                                    ack_message.broken();
                                }
                            }
                            log::info!("发送ACK {}-{}", seq, ack);
                            if let Ok(nbytes) = socket_lock.send(&ack_message.to_bytes()).await {
                                assert_eq!(nbytes, 27);
                            }
                        }
                    },
                    ack_stop_msg = ack_stop_signal_receiver.recv() => {
                        if ack_stop_msg.is_some() {
                            log::info!("ACK Sender End Successfully");
                            break;
                        } else {
                            log::info!("ACK Sender End Failed");
                        }
                    }
                }
            }
        });
        let sync_packets_set_clone = sync_packets_set.clone();
        let (sync_sender, mut sync_receiver) =
            tokio::sync::mpsc::channel::<(u16, usize, usize)>(100);
        let (sync_stop_signal_sender, mut sync_stop_signal_receiver) = tokio::sync::mpsc::channel::<u32>(3);
        // 用于同步状态
        tokio::spawn(async move {
            let mut end_flag = false;
            let mut total_packets_num = 0;
            let mut sync_num = 0;
            loop {
                tokio::select! {
                    sync_msg = sync_receiver.recv() => {
                        if let Some((sync_number, seq, ack)) = sync_msg {
                            {
                                let mut sync_packets_set_lock = sync_packets_set_clone.lock().await;
                                sync_packets_set_lock.push((Instant::now(), sync_number));
                            }
                            sync_num += 1;
                            log::info!("{:?}: Sync ID {} 已同步", Instant::now(), sync_number);
                            if end_flag && total_packets_num == sync_num {
                                log::info!("Sync End Successfully");
                                ack_stop_signal_sender.send(()).await;
                                recv_stop_signal_sender.send(()).await;
                                break;
                            }
                        }
                    },
                    stop_signal = sync_stop_signal_receiver.recv() => {
                        if let Some(num) = stop_signal {
                            log::info!("Sync Collector get Stop Signal");
                            end_flag = true;
                            total_packets_num = num;
                        } else {
                            log::info!("Sync Stop Signal is broken");
                        }
                    }
                }
            }
        });

        let recv_socket_lock = self.recv_socket.lock().await;
        let mut recv_buf = vec![0u8; SYNC_PACKET_MAX_SIZE];
        loop {
            tokio::select! {
                recv_bytes = recv_socket_lock.recv(&mut recv_buf) => {
                    // 假设不会分片
                    if let Ok(nbytes) = recv_bytes {
                        assert!(nbytes >= 27);
                        if let Some(message) = Message::from_bytes(&mut recv_buf[..nbytes]) {
                            let (type_id, recv_wnd, sync_number, seq, ack, timestamp, rto) =
                                message.get_id_wnd_sync_seq_ack_timestamp_rto_field();
                            log::info!("Recv Packet {}-{}", seq, ack);
                            let payload = message.get_payload();
                            if total_packets_num == 0 {
                                total_packets_num = message.get_total_packets_num();
                                sync_stop_signal_sender.send(total_packets_num as u32).await;
                            }
                            // 发送ACK
                            ack_sender
                                .send((sync_number, seq, ack, timestamp, rto))
                                .await;
                            if duplicated_packet_table.contains_key(&sync_number) {
                                let duplicated_sender = duplicated_packet_table.get(&sync_number).unwrap();
                                duplicated_sender.send(()).await;
                            } else {
                                payloads.push(((seq, ack), payload));
                                let (duplicated_sender, duplicated_receiver) =
                                    tokio::sync::mpsc::channel::<()>(3);
                                duplicated_packet_table.insert(sync_number, duplicated_sender);
                                let sync_manager = SyncManager::new(sync_number, rto);
                                let sync_future = sync_manager.start_recv_tracking(
                                    (seq as usize, ack as usize),
                                    duplicated_receiver,
                                    ack_sender.clone(),
                                    sync_sender.clone(),
                                );
                                tokio::spawn(async move {
                                    sync_future.await;
                                });
                            }
                        } else {
                            log::info!("Not a Message");
                        }
                    }
                },
                recv_stop_msg = recv_stop_signal_receiver.recv() => {
                    if recv_stop_msg.is_some() {
                        log::info!("Recv Coroutine Stop Successfully");
                        break;
                    }
                }
            }
        }
        // 同步与发送结束，整理包
        payloads.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        let mut index = 0;
        let len = buf.len();
        for (_, payload) in payloads {
            for byte in payload {
                buf[index] = byte;
                index += 1;
                if index == len {
                    break;
                }
            }
            if index == len {
                break;
            }
        }
        let mut sync_packets_set_lock = sync_packets_set.lock().await;
        sync_packets_set_lock.sort_by(|a, b| a.1.cmp(&b.1));
        let mut recv_file = File::create("./send_recv_data.txt")?;
        let mut file_writer = BufWriter::new(recv_file);
        for pair in sync_packets_set_lock.iter() {
            file_writer.write(format!("{}:{:?}\n", pair.1, pair.0).as_bytes());
        }
        Ok(std::cmp::max(len, total_packets_num as usize))
    }
}

pub fn split_frame(packet_length: usize, frame_size: usize) -> Vec<(usize, usize)> {
    let mut frames = Vec::new();
    let mut begin_idx = 0usize;
    let mut end_idx = frame_size;
    while end_idx < packet_length {
        frames.push((begin_idx, end_idx));
        begin_idx = end_idx;
        end_idx += frame_size;
    }
    frames.push((begin_idx, packet_length));
    frames
}
