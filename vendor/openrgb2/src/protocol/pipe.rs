//! Pipelined LED updates (TuxRGB patch).
//!
//! On SDK v6 the server acknowledges every packet, after the controller's
//! write that is in progress. Waiting for the acknowledge of every
//! controller before the next frame ties all of them to the slowest one:
//! with SMBus RAM or an I2C graphics card (hundreds of milliseconds per
//! write), a mouse or a motherboard header updated once a second, or less.
//!
//! [`LedPipe`] is a connection of its own for colours: packets are written
//! as soon as asked, acknowledges are read by a separate task and handed
//! back with their controller id. The caller keeps at most one packet in
//! flight per controller, so every controller is updated as fast as it
//! can be, independently, without a queue building up on the server.

use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::AsyncReadExt;
use tokio::net::ToSocketAddrs;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::data::{Color, ModeData, RawString};
use crate::{
    Acknowledge, DeserFromBuf, OpenRgbError, OpenRgbMessageHeader, OpenRgbResult,
    OpenRgbWritePacket, PacketId, ProtocolStream, ReceivedMessage, SerToBuf, WriteMessage,
};

/// The server acknowledged a packet for a controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipeAck {
    /// Controller id the packet was for.
    pub controller_id: u32,
    /// `false` if the server refused the packet (unknown controller, bad data).
    pub ok: bool,
}

/// Connection dedicated to LED / mode-colour updates, see the module docs.
pub struct LedPipe {
    writer: OwnedWriteHalf,
    protocol_version: u32,
    acks: mpsc::UnboundedReceiver<OpenRgbResult<PipeAck>>,
    reader: JoinHandle<()>,
    buf: Vec<u8>,
    list_updated: Arc<AtomicBool>,
}

impl Debug for LedPipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedPipe")
            .field("protocol_version", &self.protocol_version)
            .finish_non_exhaustive()
    }
}

impl Drop for LedPipe {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl LedPipe {
    /// Connects and negotiates SDK v6 (acknowledges are required).
    ///
    /// # Errors
    ///
    /// Connection failure, or a server older than SDK v6.
    pub async fn connect(
        addr: impl ToSocketAddrs + Debug + Copy,
        client_name: &str,
    ) -> OpenRgbResult<Self> {
        let mut stream = ProtocolStream::connect(addr, crate::protocol::DEFAULT_PROTOCOL)
            .await
            .map_err(|source| OpenRgbError::ConnectionError {
                addr: format!("{addr:?}"),
                source,
            })?;
        let server: u32 = stream
            .request(
                0,
                PacketId::RequestProtocolVersion,
                &crate::protocol::DEFAULT_PROTOCOL,
            )
            .await?;
        let protocol_version = crate::protocol::DEFAULT_PROTOCOL.min(server);
        if protocol_version < 6 {
            return Err(OpenRgbError::ProtocolError(format!(
                "pipelined updates need SDK v6, the server speaks v{protocol_version}"
            )));
        }
        stream.set_protocol_version(protocol_version);
        stream
            .write(0, PacketId::SetClientName, &RawString(client_name))
            .await?;
        let (mut read, writer) = stream.into_tcp().into_split();
        let (tx, acks) = mpsc::unbounded_channel();
        let list_updated = Arc::new(AtomicBool::new(false));
        let announced = list_updated.clone();
        let reader = tokio::spawn(async move {
            let mut scratch = vec![0u8; 4096];
            loop {
                let result = async {
                    let header = OpenRgbMessageHeader::read(&mut read).await?;
                    header.check_size()?;
                    let size = header.packet_size as usize;
                    if scratch.len() < size.min(64 * 1024) {
                        scratch.resize(size.min(64 * 1024), 0);
                    }
                    if header.packet_id == PacketId::Acknowledge {
                        let mut data = vec![0u8; size];
                        read.read_exact(&mut data).await?;
                        let mut msg = ReceivedMessage::new(&data, protocol_version);
                        let ack = Acknowledge::deserialize(&mut msg)?;
                        return Ok(Some(PipeAck {
                            controller_id: header.device_id,
                            ok: ack.status_code.is_ok(),
                        }));
                    }
                    // Notifications (one per controller update on SDK v6)
                    // are not needed here: skipped without allocating. A
                    // new device list is noted (hot-plug).
                    if header.packet_id == PacketId::DeviceListUpdated {
                        announced.store(true, Ordering::Release);
                    }
                    let mut left = size;
                    while left > 0 {
                        let n = left.min(scratch.len());
                        read.read_exact(&mut scratch[..n]).await?;
                        left -= n;
                    }
                    Ok::<_, OpenRgbError>(None)
                }
                .await;
                match result {
                    Ok(Some(ack)) => {
                        if tx.send(Ok(ack)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                }
            }
        });
        Ok(Self {
            writer,
            protocol_version,
            acks,
            reader,
            buf: Vec::new(),
            list_updated,
        })
    }

    async fn send<T: SerToBuf>(
        &mut self,
        controller_id: u32,
        packet_id: PacketId,
        data: T,
    ) -> OpenRgbResult<()> {
        use tokio::io::AsyncWriteExt;
        let mut body = WriteMessage::new(self.protocol_version);
        data.serialize(&mut body)?;
        let header = OpenRgbMessageHeader {
            packet_id,
            device_id: controller_id,
            packet_size: body.len() as u32,
        };
        // Header and body in one write: one segment on the loopback.
        self.buf.clear();
        let mut head = WriteMessage::with_capacity(0, 16);
        head.write_slice(&OpenRgbMessageHeader::MAGIC);
        head.write_u32(header.device_id);
        head.write_value(header.packet_id)?;
        head.write_u32(header.packet_size);
        self.buf.extend_from_slice(head.bytes());
        self.buf.extend_from_slice(body.bytes());
        self.writer.write_all(&self.buf).await?;
        Ok(())
    }

    /// Sends the colours of every LED of a controller (no waiting: the
    /// acknowledge arrives through [`LedPipe::next_ack`]).
    ///
    /// # Errors
    ///
    /// The connection is broken.
    pub async fn update_leds(&mut self, controller_id: u32, colors: &[Color]) -> OpenRgbResult<()> {
        self.send(
            controller_id,
            PacketId::RGBControllerUpdateLeds,
            &OpenRgbWritePacket::new(colors),
        )
        .await
    }

    /// Sends a mode (single-colour controllers: the colour lives in the mode).
    ///
    /// # Errors
    ///
    /// The connection is broken.
    pub async fn update_mode(&mut self, controller_id: u32, mode: &ModeData) -> OpenRgbResult<()> {
        self.send(
            controller_id,
            PacketId::RGBControllerUpdateMode,
            &OpenRgbWritePacket::new((mode.id() as u32, mode)),
        )
        .await
    }

    /// `true` once after the server announced a new device list.
    pub fn take_list_updated(&self) -> bool {
        self.list_updated.swap(false, Ordering::AcqRel)
    }

    /// Next acknowledge (cancellation safe). `None` once the reader stopped.
    pub async fn next_ack(&mut self) -> Option<OpenRgbResult<PipeAck>> {
        self.acks.recv().await
    }
}
