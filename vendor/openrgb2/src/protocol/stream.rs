use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{pin::Pin, time::Duration};

use crate::{Acknowledge, OpenRgbError::ProtocolError, protocol::PacketId};
use crate::{DeserFromBuf, OpenRgbError, OpenRgbResult, ReceivedMessage, SerToBuf, WriteMessage};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpStream, ToSocketAddrs},
};

#[derive(Debug)]
pub(crate) struct RecvPacket {
    pub header: OpenRgbMessageHeader,
    data: Vec<u8>,
}

impl std::fmt::Display for RecvPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?} {:?} ({})",
            self.header.packet_id,
            self.data,
            String::from_utf8_lossy(&self.data)
        )
    }
}

impl RecvPacket {
    async fn read_data(
        stream: &mut TcpStream,
        header: &OpenRgbMessageHeader,
    ) -> OpenRgbResult<Vec<u8>> {
        // the header tells us exactly how long the packet is, so we might as well read it all at once
        header.check_size()?;
        let mut data = vec![0; header.packet_size as usize];
        stream.read_exact(&mut data).await?;
        Ok(data)
    }

}

/// Utility struct to write packets.
/// Some packets need to be prepended by their length.
/// This struct serializes the contents and prepends the length to the buffer.
#[derive(Debug)]
pub(crate) struct OpenRgbWritePacket<T: SerToBuf> {
    pub contents: T,
}

impl<T: SerToBuf> OpenRgbWritePacket<T> {
    pub fn new(contents: T) -> OpenRgbWritePacket<T> {
        Self { contents }
    }
}

impl<T: SerToBuf> SerToBuf for OpenRgbWritePacket<T> {
    fn serialize(&self, buf: &mut WriteMessage) -> OpenRgbResult<()> {
        let mut inner_buf = WriteMessage::new(buf.protocol_version());
        self.contents.serialize(&mut inner_buf)?;
        let len = inner_buf.len() + size_of::<u32>(); // + u32 to account for the length field itself
        buf.write_u32(len as u32);
        buf.write_slice(inner_buf.bytes());
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct OpenRgbMessageHeader {
    pub packet_id: PacketId,
    pub device_id: u32,
    pub packet_size: u32,
}

impl DeserFromBuf for OpenRgbMessageHeader {
    fn deserialize(buf: &mut ReceivedMessage<'_>) -> OpenRgbResult<Self>
    where
        Self: Sized,
    {
        let magic = buf.read_value::<[u8; 4]>()?;
        if magic != Self::MAGIC {
            return Err(OpenRgbError::ProtocolError(format!(
                "expected OpenRGB magic value, got {magic:?}"
            )));
        }

        let device_id = buf.read_value::<u32>()?;
        let packet_id = buf.read_value::<PacketId>()?;
        let packet_size = buf.read_value::<u32>()?;
        Ok(Self {
            device_id,
            packet_id,
            packet_size,
        })
    }
}

impl OpenRgbMessageHeader {
    pub(crate) const MAGIC: [u8; 4] = *b"ORGB";

    /// Largest packet accepted (TuxRGB patch). Real packets are at most a
    /// few hundred kilobytes (a big keyboard's description); a corrupted or
    /// desynchronised stream could otherwise announce up to 4 GiB and have
    /// the client allocate it.
    pub(crate) const MAX_PACKET_SIZE: u32 = 64 * 1024 * 1024;

    pub(crate) fn check_size(&self) -> OpenRgbResult<()> {
        if self.packet_size > Self::MAX_PACKET_SIZE {
            return Err(OpenRgbError::ProtocolError(format!(
                "packet {} announces {} bytes, over the {} bytes limit",
                self.packet_id,
                self.packet_size,
                Self::MAX_PACKET_SIZE
            )));
        }
        Ok(())
    }

    async fn read(stream: &mut TcpStream) -> OpenRgbResult<Self> {
        // header is always 16 bytes long
        let mut buf = [0u8; 16];
        stream.read_exact(&mut buf).await?;
        let mut recv = ReceivedMessage::new(&buf, 0); // header is constant across protocol versions
        Self::deserialize(&mut recv)
    }

    async fn write(&self, stream: &mut TcpStream) -> OpenRgbResult<()> {
        let mut buf = WriteMessage::with_capacity(0, 16);
        buf.write_slice(&Self::MAGIC);
        buf.write_u32(self.device_id);
        buf.write_value(self.packet_id)?;
        buf.write_u32(self.packet_size);
        tracing::trace!("Writing header: {:?}", self);
        tracing::trace!("Writing header: {}", buf);
        stream.write_all(buf.bytes()).await?;
        Ok(())
    }
}

/// `tokio TcpStream` with an `OpenRGB` protocol version.
/// The version is tagged to all received and written packets, since packet format depends on protocol version.
pub(crate) struct ProtocolStream {
    stream: TcpStream,
    protocol_version: u32,
    /// Set when the server announced a change of its device list
    /// (`DeviceListUpdated`, TuxRGB patch): hot-plugged devices are noticed
    /// right away instead of at the next poll.
    pub(crate) list_updated: Arc<AtomicBool>,
    /// Reused buffer for the server notifications that are skipped (one
    /// `RGBControllerSignalUpdate` per controller and frame on SDK v6): they
    /// are discarded without an allocation each (TuxRGB patch).
    scratch: Vec<u8>,
}

impl ProtocolStream {
    pub async fn connect<A: ToSocketAddrs>(
        addr: A,
        protocol_version: u32,
    ) -> std::io::Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        // Small packets (LED frames) go out at once.
        let _ = stream.set_nodelay(true);
        Ok(Self {
            stream,
            protocol_version,
            list_updated: Arc::default(),
            scratch: Vec::new(),
        })
    }

    pub fn protocol_version(&self) -> u32 {
        self.protocol_version
    }

    pub fn set_protocol_version(&mut self, version: u32) {
        self.protocol_version = version;
    }

    /// Writes and receives a packet. Use this for packets that expect a response that is not an ACK.
    pub async fn request<I: SerToBuf, O: DeserFromBuf>(
        &mut self,
        device_id: u32,
        packet_id: PacketId,
        data: I,
    ) -> OpenRgbResult<O> {
        self.write_packet(device_id, packet_id, data).await?;
        let read = self.read_packet(device_id, packet_id).await?;
        self.recv_ack(device_id, packet_id).await?;
        Ok(read)
    }

    /// Writes and receives a packet. Use this for packets that don't expect a response (excluding ACK).
    pub async fn write<I: SerToBuf>(
        &mut self,
        device_id: u32,
        packet_id: PacketId,
        data: I,
    ) -> OpenRgbResult<()> {
        self.write_packet(device_id, packet_id, data).await?;
        self.recv_ack(device_id, packet_id).await?;
        Ok(())
    }

    pub async fn write_multiple<I: SerToBuf>(
        &mut self,
        packets: impl IntoIterator<Item = (u32, PacketId, I)>,
    ) -> OpenRgbResult<()> {
        let mut ids = Vec::new();
        for (device_id, packet_id, d) in packets {
            self.write_packet(device_id, packet_id, d).await?;
            ids.push((device_id, packet_id))
        }

        while !ids.is_empty() {
            let header = self.recv_header().await?;

            if header.packet_id.is_server_only() {
                self.skip_notification(&header).await?;
                continue;
            }

            // Acknowledges are tiny: read them like any packet.
            let _ = RecvPacket::read_data(&mut self.stream, &header).await?;

            if header.packet_id != PacketId::Acknowledge {
                return Err(ProtocolError(format!(
                    "Expected acknowledge but received {}",
                    header.packet_id
                )));
            }

            if !ids.iter().any(|(d_id, _)| *d_id == header.device_id) {
                return Err(ProtocolError(format!(
                    "Received packet has invalid device id, received {} but expected one of {:?}",
                    header.device_id, ids
                )));
            }

            ids.retain(|(d_id, _)| *d_id != header.device_id);
            // todo: check ack status
        }
        Ok(())
    }

    #[allow(unused, reason = "might be used later")]
    async fn recv_ack(&mut self, device_id: u32, packet_id: PacketId) -> OpenRgbResult<()> {
        if self.protocol_version < 6 {
            return Ok(()); // only applies to protocol version 6 and up
        }

        let ack = self
            .read_packet::<Acknowledge>(device_id, PacketId::Acknowledge)
            .await?;

        if !ack.status_code.is_ok() {
            return Err(OpenRgbError::ProtocolError(format!(
                "Acknowledge returned error: {:?}",
                ack
            )));
        }

        if ack.packet_id != packet_id {
            return Err(OpenRgbError::ProtocolError(format!(
                "Received acknowledge contains unexpected packet id. Expected {}, got {}",
                packet_id, ack.packet_id
            )));
        }
        Ok(())
    }

    async fn read_packet<T: DeserFromBuf>(
        &mut self,
        device_id: u32,
        packet_id: PacketId,
    ) -> OpenRgbResult<T> {
        loop {
            // Keep receiving packets until we get one that we don't ignore. This is likely the response we're looking for
            let expected = packet_id.expected_response();
            let packet = tokio::time::timeout(Duration::from_secs(1), async {
                let header = self.recv_header().await?;
                if header.packet_id.is_server_only() && header.packet_id != expected {
                    tracing::trace!(
                        "Received {} instead of expected {}, ignoring...",
                        header.packet_id,
                        expected
                    );
                    self.skip_notification(&header).await?;
                    return Ok(None);
                }
                let data = RecvPacket::read_data(&mut self.stream, &header).await?;
                Ok::<_, OpenRgbError>(Some(RecvPacket { header, data }))
            })
            .await??;
            let Some(packet) = packet else {
                continue;
            };

            let mut recv = ReceivedMessage::new(&packet.data, self.protocol_version());
            // tracing::debug!("Read packet data: {}", recv);
            match packet.header.packet_id {
                p if p != expected => {
                    return Err(OpenRgbError::ProtocolError(format!(
                        "Unexpected packet ID: expected {}, got {}",
                        expected, packet.header.packet_id
                    )));
                }
                _ => {
                    if packet.header.device_id != device_id {
                        return Err(OpenRgbError::ProtocolError(format!(
                            "Unexpected device ID: expected {}, got {}",
                            device_id, packet.header.device_id
                        )));
                    }

                    return T::deserialize(&mut recv);
                }
            }
        }
    }

    /// Reads the next packet header (size checked).
    async fn recv_header(&mut self) -> OpenRgbResult<OpenRgbMessageHeader> {
        let header = OpenRgbMessageHeader::read(&mut self.stream).await?;
        header.check_size()?;
        Ok(header)
    }

    /// Discards the body of a server notification nobody waits for, noting
    /// device list changes and controller layout changes (zones resized,
    /// segments, hidden devices: everything but colours and modes).
    async fn skip_notification(&mut self, header: &OpenRgbMessageHeader) -> OpenRgbResult<()> {
        if header.packet_id == PacketId::DeviceListUpdated {
            self.list_updated.store(true, Ordering::Release);
        }
        let mut left = header.packet_size as usize;
        if left > 0 && self.scratch.len() < left.min(64 * 1024) {
            self.scratch.resize(left.min(64 * 1024), 0);
        }
        let mut first = true;
        while left > 0 {
            let n = left.min(self.scratch.len());
            self.stream.read_exact(&mut self.scratch[..n]).await?;
            if first && header.packet_id == PacketId::RGBControllerSignalUpdate && n >= 8 {
                // [u32 size][u32 reason]: 0 = LEDs, 1 = mode, 2 = mode saved.
                let reason = u32::from_le_bytes([
                    self.scratch[4],
                    self.scratch[5],
                    self.scratch[6],
                    self.scratch[7],
                ]);
                if reason >= 3 {
                    self.list_updated.store(true, Ordering::Release);
                }
            }
            first = false;
            left -= n;
        }
        Ok(())
    }

    pub(crate) async fn write_packet<T: SerToBuf>(
        &mut self,
        device_id: u32,
        packet_id: PacketId,
        data: T,
    ) -> OpenRgbResult<()> {
        let mut buf = WriteMessage::new(self.protocol_version());
        data.serialize(&mut buf)?;
        let packet_size = buf.len() as u32;
        let header = OpenRgbMessageHeader {
            packet_id,
            device_id,
            packet_size,
        };
        header.write(&mut self.stream).await?;

        tracing::trace!("Writing packet: {}", buf);
        self.stream.write_all(buf.bytes()).await?;
        Ok(())
    }
}

impl AsyncRead for ProtocolStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let pin = Pin::new(&mut self.stream);
        AsyncRead::poll_read(pin, cx, buf)
    }
}

impl AsyncWrite for ProtocolStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let pin = Pin::new(&mut self.get_mut().stream);
        AsyncWrite::poll_write(pin, cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let pin = Pin::new(&mut self.get_mut().stream);
        AsyncWrite::poll_flush(pin, cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let pin = Pin::new(&mut self.get_mut().stream);
        AsyncWrite::poll_shutdown(pin, cx)
    }
}
