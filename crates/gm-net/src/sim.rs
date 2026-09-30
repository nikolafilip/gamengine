//! quinn over turmoil's simulated UDP, with deterministic per-datagram loss injection.
//! turmoil's own `fail_rate` partitions whole links; PROTOCOL.md 9 needs independent 3%
//! datagram loss, so the socket drops on send with its own seeded generator.

use std::future::Future;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};

type ReadyFuture = Pin<Box<dyn Future<Output = io::Result<()>> + Send>>;

/// xorshift64*: enough for loss injection, no dependency.
#[derive(Debug)]
struct Rng(u64);

impl Rng {
    fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A turmoil UDP socket usable by quinn.
pub struct TurmoilSocket {
    inner: Arc<turmoil::net::UdpSocket>,
    loss: f64,
    rng: Mutex<Rng>,
    readable: Mutex<Option<ReadyFuture>>,
    pub sent: AtomicU64,
    pub dropped: AtomicU64,
}

impl std::fmt::Debug for TurmoilSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurmoilSocket")
            .field("loss", &self.loss)
            .field("sent", &self.sent.load(Ordering::Relaxed))
            .field("dropped", &self.dropped.load(Ordering::Relaxed))
            .finish()
    }
}

impl TurmoilSocket {
    /// Bind inside a turmoil host. `loss` is the fraction of outgoing datagrams dropped.
    pub async fn bind(
        addr: impl turmoil::ToSocketAddrs,
        loss: f64,
        seed: u64,
    ) -> io::Result<Arc<TurmoilSocket>> {
        let inner = turmoil::net::UdpSocket::bind(addr).await?;
        Ok(Arc::new(TurmoilSocket {
            inner: Arc::new(inner),
            loss,
            rng: Mutex::new(Rng(seed | 1)),
            readable: Mutex::new(None),
            sent: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }))
    }

    fn drop_this_one(&self) -> bool {
        self.loss > 0.0 && self.rng.lock().expect("rng lock").next_f64() < self.loss
    }

    fn send_one(&self, dest: SocketAddr, bytes: &[u8]) {
        self.sent.fetch_add(1, Ordering::Relaxed);
        if self.drop_this_one() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // A refused send (peer host down) is indistinguishable from loss on a real network.
        let _ = self.inner.try_send_to(bytes, dest);
    }
}

#[derive(Debug)]
struct AlwaysWritable;

impl UdpPoller for AlwaysWritable {
    fn poll_writable(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncUdpSocket for TurmoilSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(AlwaysWritable)
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        match transmit.segment_size {
            Some(seg) if seg > 0 => {
                for chunk in transmit.contents.chunks(seg) {
                    self.send_one(transmit.destination, chunk);
                }
            }
            _ => self.send_one(transmit.destination, transmit.contents),
        }
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        loop {
            match self.inner.try_recv_from(&mut bufs[0]) {
                Ok((n, addr)) => {
                    meta[0] = RecvMeta {
                        addr,
                        len: n,
                        stride: n,
                        ..RecvMeta::default()
                    };
                    return Poll::Ready(Ok(1));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    let mut slot = self.readable.lock().expect("readable lock");
                    if slot.is_none() {
                        let sock = self.inner.clone();
                        *slot = Some(Box::pin(async move {
                            sock.readable().await.map_err(io::Error::other)
                        }));
                    }
                    match slot.as_mut().expect("just set").as_mut().poll(cx) {
                        Poll::Ready(Ok(())) => {
                            *slot = None;
                            continue;
                        }
                        Poll::Ready(Err(e)) => {
                            *slot = None;
                            return Poll::Ready(Err(e));
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    fn may_fragment(&self) -> bool {
        // Keeps quinn at its initial MTU; the simulation never fragments anyway.
        true
    }
}
