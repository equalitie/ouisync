//! In its own test binary (process) because it exhausts the process' file descriptors.
#![cfg(unix)]

use ouisync_service::{Service, transport::local};
use std::{fs::File, net::Ipv4Addr, time::Duration};
use tempfile::TempDir;
use tokio::{fs, net::TcpStream, select, task, time};

// Temporary `accept` errors must neither stop the service (the remote listener) nor drop the
// connections waiting to be accepted (the local listener).
//
// The test simulates temporarily running out of file descriptors because that's easy to reproduce,
// but other temporary errors cause the same issue: running out of other resources (e.g. `ENFILE`,
// `ENOBUFS`, `ENOMEM`) and errors of an individual pending connection, which don't affect the
// listener (e.g. `ECONNABORTED` when a client gives up before being accepted, `EPROTO`, network or
// host unreachable).
#[tokio::test]
async fn accept_recovers_from_running_out_of_file_descriptors() {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("config");
    let remote_port = free_port();
    setup_remote_control(&config_dir, remote_port).await;

    let mut service = Service::init(config_dir).await.unwrap();
    let local_endpoint = *service.local_endpoint();

    // Connect before running out of file descriptors. The connections wait in the listeners'
    // backlogs until accepted.
    let local_client = task::spawn(local::connect(local_endpoint));
    let remote_client = TcpStream::connect((Ipv4Addr::LOCALHOST, remote_port))
        .await
        .unwrap();
    time::sleep(Duration::from_millis(100)).await;

    // Lower the limit (to not open too many files) and use up all the remaining file descriptors.
    let limit = get_file_limit();
    set_file_limit(libc::rlimit {
        rlim_cur: 256,
        ..limit
    });

    let mut files = Vec::new();
    while let Ok(file) = File::open("/dev/null") {
        files.push(file);
    }

    // Accepting fails with EMFILE, but the service keeps running.
    select! {
        result = service.run() => panic!("service stopped: {result:?}"),
        _ = time::sleep(Duration::from_millis(500)) => (),
    }

    drop(files);
    set_file_limit(limit);

    // The local client waiting in the backlog gets accepted (and authenticated).
    select! {
        result = service.run() => panic!("service stopped: {result:?}"),
        result = time::timeout(Duration::from_secs(5), local_client) => {
            result.unwrap().unwrap().unwrap();
        }
    }

    // The remote client never starts the TLS handshake, which would block `close` otherwise.
    drop(remote_client);

    service.close().await;
}

async fn setup_remote_control(config_dir: &std::path::Path, port: u16) {
    let cert_key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();

    fs::create_dir_all(config_dir).await.unwrap();
    fs::write(config_dir.join("cert.pem"), cert_key.cert.pem())
        .await
        .unwrap();
    fs::write(
        config_dir.join("key.pem"),
        cert_key.signing_key.serialize_pem(),
    )
    .await
    .unwrap();
    fs::write(
        config_dir.join("remote_control.conf"),
        format!("\"127.0.0.1:{port}\""),
    )
    .await
    .unwrap();
}

fn free_port() -> u16 {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn get_file_limit() -> libc::rlimit {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
        0
    );
    limit
}

fn set_file_limit(limit: libc::rlimit) {
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
}
