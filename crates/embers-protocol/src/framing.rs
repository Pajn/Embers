use embers_core::RequestId;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::codec::ProtocolError;

pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;
const FRAME_HEADER_LEN: usize = 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameType {
    Request = 1,
    Response = 2,
    Event = 3,
}

impl TryFrom<u8> for FrameType {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Request),
            2 => Ok(Self::Response),
            3 => Ok(Self::Event),
            other => Err(ProtocolError::InvalidFrameType(other)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawFrame {
    pub frame_type: FrameType,
    pub request_id: RequestId,
    pub payload: Vec<u8>,
}

impl RawFrame {
    pub fn new(frame_type: FrameType, request_id: RequestId, payload: Vec<u8>) -> Self {
        Self {
            frame_type,
            request_id,
            payload,
        }
    }
}

pub async fn read_frame<R>(reader: &mut R) -> Result<Option<RawFrame>, ProtocolError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0_u8; FRAME_HEADER_LEN];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }

    let length = u32::from_le_bytes(header[0..4].try_into().expect("length bytes")) as usize;
    if length > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge(length));
    }

    let frame_type = FrameType::try_from(header[4])?;
    let request_id = RequestId(u64::from_le_bytes(
        header[5..13].try_into().expect("request id bytes"),
    ));

    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;

    Ok(Some(RawFrame {
        frame_type,
        request_id,
        payload,
    }))
}

pub async fn write_frame<W>(writer: &mut W, frame: &RawFrame) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    if frame.payload.len() > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge(frame.payload.len()));
    }

    writer
        .write_all(&(frame.payload.len() as u32).to_le_bytes())
        .await?;
    writer.write_all(&[frame.frame_type as u8]).await?;
    writer
        .write_all(&u64::from(frame.request_id).to_le_bytes())
        .await?;
    writer.write_all(&frame.payload).await?;
    writer.flush().await?;
    Ok(())
}
