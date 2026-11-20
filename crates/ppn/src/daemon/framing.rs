use std::io;

use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const FRAME_LIMIT: usize = 1024 * 1024;

pub async fn read<T: DeserializeOwned>(stream: impl AsyncRead + Unpin) -> io::Result<T> {
    let mut frame = Vec::new();
    let mut reader = BufReader::new(stream).take(FRAME_LIMIT as u64);
    if reader.read_until(b'\n', &mut frame).await? == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    if frame.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Expected one bounded JSON line",
        ));
    }
    serde_json::from_slice(&frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub async fn write<T: Serialize>(mut stream: impl AsyncWrite + Unpin, value: &T) -> io::Result<()> {
    let mut frame = serde_json::to_vec(value)?;
    if frame.len() >= FRAME_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Control response exceeds the frame limit",
        ));
    }
    frame.push(b'\n');
    stream.write_all(&frame).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[tokio::test]
    async fn reads_one_complete_frame_at_a_time() {
        let (mut client, mut server) = tokio::io::duplex(128);
        assert_eq!(
            read::<Value>(tokio::io::empty()).await.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        let request = json!({"version": 1, "op": "status"});
        write(&mut client, &request).await.unwrap();
        assert_eq!(read::<Value>(&mut server).await.unwrap(), request);
        client
            .write_all(br#"{"version":1,"op":"nudge"}"#)
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        assert_eq!(
            read::<Value>(&mut server).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
