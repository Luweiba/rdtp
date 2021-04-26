use crc::crc32;
/// 定义网络传输的包的格式
/// 区分数据包与控制信息包的类型: Message Type ID
/// RTT的计算需要的字段: TimeStamp
/// 可靠传输防止失序的字段: Sequence Number
/// Type ID: u8
/// Payload Length: u32
/// TimeStamp: u32
/// : 本地维护一个时间Instant，起始值为 0
/// : 每次收到一个ACK就计算新的RTT，然后更新RTO
///
/// # Message Format
/// `Type ID[1 byte]``Sequence Number[2 bytes]``TimeStamp[4 bytes]``Payload Length[4 bytes]``Payload Content[..]`

pub struct Message {
    /// # Type ID
    /// 0 => data message
    /// 1 => ACK [Sequence Number to identify]
    type_id: u8,
    /// 发送包的总数，用于接收方确认停止接收
    packets_num: u16,
    /// 窗口大小
    recv_wnd: u16,
    /// 同步号
    sync_number: u16,
    /// 序列号
    seq: u32,
    /// 确认号
    ack: u32,
    /// 时间戳，用于测量RTT
    timestamp: u32,
    /// RTO 发送方超时时间（对于该包而言）
    rto: u32,
    /// 校验和，采用CRC32 IEEE来计算
    checksum: u32,
    /// 数据负载
    payload: Vec<u8>,
}

impl Message {
    pub fn new(
        type_id: u8,
        packets_num: u16,
        recv_wnd: u16,
        sync_number: u16,
        seq: u32,
        ack: u32,
        timestamp: u32,
        rto: u32,
        payload: Vec<u8>,
    ) -> Self {
        let mut raw_message_in_binary = Vec::new();
        raw_message_in_binary.push(type_id);
        raw_message_in_binary.extend_from_slice(&packets_num.to_le_bytes());
        raw_message_in_binary.extend_from_slice(&recv_wnd.to_le_bytes());
        raw_message_in_binary.extend_from_slice(&sync_number.to_le_bytes());
        raw_message_in_binary.extend_from_slice(&seq.to_le_bytes());
        raw_message_in_binary.extend_from_slice(&ack.to_le_bytes());
        raw_message_in_binary.extend_from_slice(&timestamp.to_le_bytes());
        raw_message_in_binary.extend_from_slice(&rto.to_le_bytes());
        raw_message_in_binary.extend_from_slice(&payload);
        let mut checksum = crc32::checksum_ieee(&raw_message_in_binary);
        Self {
            type_id,
            packets_num,
            recv_wnd,
            sync_number,
            seq,
            ack,
            timestamp,
            rto,
            checksum,
            payload,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut message_in_binary = Vec::new();
        message_in_binary.push(self.type_id);
        message_in_binary.extend_from_slice(&self.packets_num.to_le_bytes());
        message_in_binary.extend_from_slice(&self.recv_wnd.to_le_bytes());
        message_in_binary.extend_from_slice(&self.sync_number.to_le_bytes());
        message_in_binary.extend_from_slice(&self.seq.to_le_bytes());
        message_in_binary.extend_from_slice(&self.ack.to_le_bytes());
        message_in_binary.extend_from_slice(&self.timestamp.to_le_bytes());
        message_in_binary.extend_from_slice(&self.rto.to_le_bytes());
        message_in_binary.extend_from_slice(&self.checksum.to_le_bytes());
        message_in_binary.extend_from_slice(&self.payload);
        message_in_binary
    }

    pub fn broken(&mut self) {
        self.checksum = 0;
    }

    pub fn from_bytes(message_bytes: &[u8]) -> Option<Self> {
        let message_len = message_bytes.len();
        if message_len < 27 {
            return None;
        }
        let type_id = message_bytes[0];
        let packets_num_buf = [message_bytes[1], message_bytes[2]];
        let packets_num = u16::from_le_bytes(packets_num_buf);
        let recv_wnd_buf = [message_bytes[3], message_bytes[4]];
        let recv_wnd = u16::from_le_bytes(recv_wnd_buf);
        let sync_number_buf = [message_bytes[5], message_bytes[6]];
        let sync_number = u16::from_le_bytes(sync_number_buf);
        let seq_buf = [
            message_bytes[7],
            message_bytes[8],
            message_bytes[9],
            message_bytes[10],
        ];
        let seq = u32::from_le_bytes(seq_buf);
        let ack_buf = [
            message_bytes[11],
            message_bytes[12],
            message_bytes[13],
            message_bytes[14],
        ];
        let ack = u32::from_le_bytes(ack_buf);
        let timestamp_buf = [
            message_bytes[15],
            message_bytes[16],
            message_bytes[17],
            message_bytes[18],
        ];
        let timestamp = u32::from_le_bytes(timestamp_buf);
        let rto_buf = [
            message_bytes[19],
            message_bytes[20],
            message_bytes[21],
            message_bytes[22],
        ];
        let rto = u32::from_le_bytes(rto_buf);
        let checksum_buf = [
            message_bytes[23],
            message_bytes[24],
            message_bytes[25],
            message_bytes[26],
        ];
        let checksum = u32::from_le_bytes(checksum_buf);
        // 验证checksum
        let mut message_checked = message_bytes[..23].to_vec();
        message_checked.extend_from_slice(&message_bytes[27..]);
        let checksum_checked = crc32::checksum_ieee(&message_checked);
        if checksum_checked != checksum {
            println!("checked: {}, current: {}", checksum_checked, checksum);
            println!("Packet Broken");
            return None;
        }
        let payload = message_bytes[27..].to_vec();
        Some(Self {
            type_id,
            packets_num,
            recv_wnd,
            sync_number,
            seq,
            ack,
            timestamp,
            rto,
            checksum,
            payload,
        })
    }

    pub fn get_id_wnd_sync_seq_ack_timestamp_rto_field(
        &self,
    ) -> (u8, u16, u16, u32, u32, u32, u32) {
        (
            self.type_id,
            self.recv_wnd,
            self.sync_number,
            self.seq,
            self.ack,
            self.timestamp,
            self.rto,
        )
    }
    pub fn get_total_packets_num(&self) -> u16 {
        self.packets_num
    }

    pub fn get_payload(&self) -> Vec<u8> {
        self.payload.clone()
    }
}
