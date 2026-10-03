//! Is a database answering? A protocol-level handshake from the host, no credentials needed.
//! Postgres credentials are used only for the replication lag query and never leave this module.

use skym_core::model::{DatastoreKind, DatastoreProbe};
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

const TIMEOUT: Duration = Duration::from_secs(3);

/// Where an engine listens, what to send, and how to recognize the server's answer.
struct Protocol {
    port: u16,
    hello: &'static [u8],
    answered: fn(&[u8]) -> bool,
}

fn protocol(kind: DatastoreKind) -> Option<Protocol> {
    match kind {
        DatastoreKind::Postgres => {
            Some(Protocol { port: 5432, hello: &SSL_REQUEST, answered: pg_ssl_reply })
        }
        DatastoreKind::Redis => {
            Some(Protocol { port: 6379, hello: b"PING\r\n", answered: redis_reply })
        }
        DatastoreKind::Mysql => Some(Protocol { port: 3306, hello: b"", answered: mysql_greeting }),
        DatastoreKind::Unknown => None,
    }
}

/// Read from the container's environment; deliberately neither `Serialize` nor `Display`.
pub struct PgCreds {
    user: String,
    password: Option<String>,
    db: String,
}

impl fmt::Debug for PgCreds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PgCreds {{ user: {:?}, password: <hidden>, db: {:?} }}", self.user, self.db)
    }
}

pub fn pg_creds(env: &[String]) -> PgCreds {
    let var = |name: &str| {
        env.iter().find_map(|e| e.strip_prefix(name)?.strip_prefix('=')).map(str::to_string)
    };
    let user = var("POSTGRES_USER").unwrap_or_else(|| "postgres".into());
    PgCreds {
        password: var("POSTGRES_PASSWORD"),
        db: var("POSTGRES_DB").unwrap_or(user.clone()),
        user,
    }
}

/// At the engine's default port on the container's address.
pub async fn probe(
    kind: DatastoreKind,
    ip: Option<IpAddr>,
    pg: Option<&PgCreds>,
) -> DatastoreProbe {
    let port = protocol(kind).map_or(0, |p| p.port);
    probe_at(kind, ip.map(|ip| SocketAddr::new(ip, port)), pg).await
}

async fn probe_at(
    kind: DatastoreKind,
    addr: Option<SocketAddr>,
    pg: Option<&PgCreds>,
) -> DatastoreProbe {
    let unreachable = |detail: &str| DatastoreProbe {
        reachable: false,
        detail: detail.into(),
        replication_lag_s: None,
    };
    let Some(p) = protocol(kind) else { return unreachable("unknown datastore") };
    let Some(addr) = addr else { return unreachable("no address") };
    // The address says what was probed, so a wrong guess is visible.
    let at = |what: &str| unreachable(&format!("{addr}: {what}"));
    match timeout(TIMEOUT, handshake(addr, p.hello)).await {
        Err(_) => at("timeout"),
        Ok(Err(e)) => at(&e.kind().to_string()),
        Ok(Ok(reply)) if !(p.answered)(&reply) => at("unexpected reply"),
        Ok(Ok(_)) => match pg.filter(|creds| creds.password.is_some()) {
            Some(creds) => lag(addr, creds).await,
            None => {
                DatastoreProbe { reachable: true, detail: "ok".into(), replication_lag_s: None }
            }
        },
    }
}

async fn handshake(addr: SocketAddr, hello: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(addr).await?;
    stream.write_all(hello).await?;
    let mut buf = vec![0; 64];
    let n = stream.read(&mut buf).await?;
    buf.truncate(n);
    Ok(buf)
}

/// Length 8, then the SSLRequest code 80877103.
const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f];

pub fn pg_ssl_reply(b: &[u8]) -> bool {
    matches!(b, [b'S' | b'N', ..])
}

/// Any RESP reply, including `-NOAUTH`.
pub fn redis_reply(b: &[u8]) -> bool {
    matches!(b, [b'+' | b'-' | b':' | b'$' | b'*', ..])
}

/// Handshake v10, or an error packet (`0xff`): either way the server answered.
pub fn mysql_greeting(b: &[u8]) -> bool {
    matches!(b, [_, _, _, _, 10 | 0xff, ..])
}

async fn lag(addr: SocketAddr, creds: &PgCreds) -> DatastoreProbe {
    let reachable =
        |detail: String, lag| DatastoreProbe { reachable: true, detail, replication_lag_s: lag };
    match timeout(TIMEOUT, query_lag(addr, creds)).await {
        Ok(Ok(lag)) => reachable("ok".into(), lag),
        Ok(Err(e)) => reachable(
            format!("lag unknown: {}", e.as_db_error().map_or("query failed", |d| d.code().code())),
            None,
        ),
        Err(_) => reachable("lag unknown: timeout".into(), None),
    }
}

async fn query_lag(
    addr: SocketAddr,
    creds: &PgCreds,
) -> Result<Option<f64>, tokio_postgres::Error> {
    let mut config = tokio_postgres::Config::new();
    config
        .hostaddr(addr.ip())
        .port(addr.port())
        .user(&creds.user)
        .dbname(&creds.db)
        .connect_timeout(TIMEOUT);
    if let Some(p) = &creds.password {
        config.password(p);
    }
    let (client, connection) = config.connect(tokio_postgres::NoTls).await?;
    let task = tokio::spawn(connection);
    let row = client
        .query_one(
            "select max(extract(epoch from replay_lag))::float8 from pg_stat_replication",
            &[],
        )
        .await;
    task.abort();
    row?.try_get(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_that_mean_a_server_answered() {
        assert!(
            pg_ssl_reply(b"N") && pg_ssl_reply(b"S") && !pg_ssl_reply(b"") && !pg_ssl_reply(b"E")
        );
        assert!(redis_reply(b"+PONG\r\n") && redis_reply(b"-NOAUTH Authentication required.\r\n"));
        assert!(!redis_reply(b"HTTP/1.1 400"));
        assert!(mysql_greeting(&[74, 0, 0, 0, 10, b'8']) && mysql_greeting(&[23, 0, 0, 0, 0xff]));
        assert!(!mysql_greeting(&[1, 0, 0]));
    }

    /// A server on localhost that answers every connection with `reply`.
    async fn fake(reply: &'static [u8]) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let mut buf = [0; 64];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(reply).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn probes_tell_answers_from_silence() {
        let probe = |kind, addr| async move { probe_at(kind, Some(addr), None).await };
        assert!(probe(DatastoreKind::Redis, fake(b"-NOAUTH\r\n").await).await.reachable);
        let http = fake(b"HTTP/1.1 400\r\n").await;
        let wrong = probe(DatastoreKind::Redis, http).await;
        assert_eq!((wrong.reachable, wrong.detail), (false, format!("{http}: unexpected reply")));
        let closed =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        assert!(!probe(DatastoreKind::Postgres, closed).await.reachable);
        assert_eq!(probe_at(DatastoreKind::Redis, None, None).await.detail, "no address");
    }

    #[tokio::test]
    async fn replication_lag_is_queried_only_with_a_password() {
        let addr = fake(b"N").await;
        let without = pg_creds(&[]);
        let with = pg_creds(&["POSTGRES_PASSWORD=x".to_string()]);
        let plain = probe_at(DatastoreKind::Postgres, Some(addr), Some(&without)).await;
        assert_eq!((plain.reachable, plain.detail.as_str()), (true, "ok"));
        let queried = probe_at(DatastoreKind::Postgres, Some(addr), Some(&with)).await;
        assert!(queried.reachable && queried.detail.starts_with("lag unknown"), "{queried:?}");
    }

    #[test]
    fn pg_credentials_from_env_stay_hidden() {
        let env = ["POSTGRES_PASSWORD=hunter2".to_string(), "POSTGRES_USER=app".to_string()];
        let creds = pg_creds(&env);
        assert_eq!((creds.user.as_str(), creds.db.as_str()), ("app", "app"));
        let debug = format!("{creds:?}");
        assert!(debug.contains("app") && !debug.contains("hunter2"), "{debug}");
        let defaults = pg_creds(&[]);
        assert_eq!((defaults.user.as_str(), defaults.password), ("postgres", None));
    }
}
