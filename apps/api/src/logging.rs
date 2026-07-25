use std::{
    env,
    io::{self, Write},
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    sync::Arc,
};

use tracing_subscriber::fmt::MakeWriter;

const MAX_DATAGRAM_BYTES: usize = 60_000;

#[derive(Clone)]
pub struct UdpLogMakeWriter {
    socket: Arc<UdpSocket>,
}

impl UdpLogMakeWriter {
    pub fn from_env() -> Result<Option<Self>, String> {
        let Some(address) = env::var("STRAYLIGHT_SYSLOG_ADDR")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        Self::connect(&address).map(Some)
    }

    fn connect(address: &str) -> Result<Self, String> {
        let addresses = address
            .to_socket_addrs()
            .map_err(|error| format!("could not resolve STRAYLIGHT_SYSLOG_ADDR: {error}"))?
            .collect::<Vec<_>>();
        let remote = addresses
            .iter()
            .copied()
            .find(SocketAddr::is_ipv4)
            .or_else(|| addresses.first().copied())
            .ok_or_else(|| "STRAYLIGHT_SYSLOG_ADDR resolved to no addresses".to_owned())?;
        let bind = if remote.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = UdpSocket::bind(bind)
            .map_err(|error| format!("could not bind the Datadog log socket: {error}"))?;
        socket
            .connect(remote)
            .map_err(|error| format!("could not connect the Datadog log socket: {error}"))?;
        socket.set_nonblocking(true).map_err(|error| {
            format!("could not make the Datadog log socket nonblocking: {error}")
        })?;
        Ok(Self {
            socket: Arc::new(socket),
        })
    }
}

impl<'a> MakeWriter<'a> for UdpLogMakeWriter {
    type Writer = UdpLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        UdpLogWriter {
            socket: Arc::clone(&self.socket),
            bytes: Vec::with_capacity(512),
        }
    }
}

pub struct UdpLogWriter {
    socket: Arc<UdpSocket>,
    bytes: Vec<u8>,
}

impl Write for UdpLogWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for UdpLogWriter {
    fn drop(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        if self.bytes.len() > MAX_DATAGRAM_BYTES {
            self.bytes = format!(
                "{{\"level\":\"warn\",\"message\":\"structured log exceeded the UDP delivery limit\",\"original_bytes\":{}}}\n",
                self.bytes.len()
            )
            .into_bytes();
        } else if !self.bytes.ends_with(b"\n") {
            self.bytes.push(b'\n');
        }
        let _ = self.socket.send(&self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Write, net::UdpSocket, time::Duration};

    use tracing_subscriber::fmt::MakeWriter;

    use super::{MAX_DATAGRAM_BYTES, UdpLogMakeWriter};

    #[test]
    fn emits_one_newline_terminated_datagram_per_event_writer() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let make_writer =
            UdpLogMakeWriter::connect(receiver.local_addr().unwrap().to_string().as_str()).unwrap();
        {
            let mut writer = make_writer.make_writer();
            writer
                .write_all(br#"{"level":"info","message":"ready"}"#)
                .unwrap();
        }
        let mut bytes = vec![0_u8; 1024];
        let count = receiver.recv(&mut bytes).unwrap();
        assert_eq!(
            br#"{"level":"info","message":"ready"}
"#,
            &bytes[..count]
        );
    }

    #[test]
    fn replaces_oversized_events_with_content_free_valid_json() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let make_writer =
            UdpLogMakeWriter::connect(receiver.local_addr().unwrap().to_string().as_str()).unwrap();
        {
            let mut writer = make_writer.make_writer();
            writer
                .write_all(&vec![b'x'; MAX_DATAGRAM_BYTES + 1])
                .unwrap();
        }
        let mut bytes = vec![0_u8; 1024];
        let count = receiver.recv(&mut bytes).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes[..count]).unwrap();
        assert_eq!(value["level"], "warn");
        assert_eq!(value["original_bytes"], MAX_DATAGRAM_BYTES + 1);
        assert!(!String::from_utf8_lossy(&bytes[..count]).contains("xxxx"));
    }
}
