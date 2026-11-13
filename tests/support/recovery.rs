//! A bounded wire proxy that loses only the execute reply after Core accepted it.

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    task::JoinSet,
    time::{sleep, timeout},
};

use super::operations::{Core, Task};

#[derive(Clone, Copy)]
pub enum LostReply {
    Disconnect,
    Timeout,
}

pub struct FaultProxy {
    pub address: String,
    accepted: Arc<AtomicUsize>,
    _task: Task,
}

// Fixture messages are fixed-length JSON, not arbitrary HTTP. Keep both headers
// and bodies bounded; do not format authentication headers or payloads on error.
async fn message(stream: &mut BufReader<TcpStream>) -> io::Result<Option<(Vec<u8>, bool)>> {
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        let remaining = 32 * 1024 - headers.len();
        if (&mut *stream)
            .take((remaining + 1) as u64)
            .read_line(&mut line)
            .await?
            == 0
        {
            return if headers.is_empty() {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
        }
        if headers.len() + line.len() > 32 * 1024 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        headers.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let execute = headers.starts_with("POST /komodo/execute ");
    let length = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .ok_or(io::ErrorKind::InvalidData)?;
    if length > 3 * 1024 * 1024 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    let mut bytes = headers.into_bytes();
    bytes.extend(body);
    Ok(Some((bytes, execute)))
}

impl FaultProxy {
    pub async fn start(core: &Core, fault: LostReply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/komodo", listener.local_addr().unwrap());
        let upstream = core
            .address
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .to_owned();
        let accepted = Arc::new(AtomicUsize::new(0));
        let captured = accepted.clone();
        let task = Task(tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let (stream,_) = connection.unwrap();
                        let upstream = upstream.clone();
                        let accepted = captured.clone();
                        connections.spawn(async move {
                            let _ = timeout(Duration::from_secs(35), async {
                                let mut client = BufReader::new(stream);
                                while let Some((request,execute)) = message(&mut client).await? {
                                    let mut server = BufReader::new(TcpStream::connect(&upstream).await?);
                                    server.get_mut().write_all(&request).await?;
                                    let (response,_) = message(&mut server).await?.ok_or(io::ErrorKind::UnexpectedEof)?;
                                    if execute {
                                        // The typed fixture has parsed, counted, persisted and
                                        // acknowledged the execution before this signal.
                                        accepted.fetch_add(1,Ordering::SeqCst);
                                        match fault { LostReply::Disconnect => (), LostReply::Timeout => sleep(Duration::from_secs(30)).await }
                                        break;
                                    }
                                    client.get_mut().write_all(&response).await?;
                                }
                                Ok::<_,io::Error>(())
                            }).await;
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => (),
                }
            }
            // Dropping the owning task also drops JoinSet and aborts connections.
        }));
        Self {
            address,
            accepted,
            _task: task,
        }
    }

    pub async fn accepted(&self, count: usize) {
        timeout(Duration::from_secs(5), async {
            while self.accepted.load(Ordering::SeqCst) < count {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
}
