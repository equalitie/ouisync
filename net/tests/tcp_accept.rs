//! In its own test binary (process) because it exhausts the process' file descriptors.
#![cfg(unix)]

use ouisync_net::{SocketOptions, tcp};
use std::{fs::File, net::Ipv4Addr, time::Duration};
use tokio::{task, time};

#[tokio::test]
async fn accept_recovers_from_running_out_of_file_descriptors() {
    let (_connector, acceptor) =
        tcp::configure((Ipv4Addr::LOCALHOST, 0).into(), SocketOptions::default()).unwrap();
    let addr = *acceptor.local_addr();

    // Connect before running out of file descriptors. The connection waits in the listener's
    // backlog until accepted.
    let _client = std::net::TcpStream::connect(addr).unwrap();

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

    let accept = task::spawn(async move { acceptor.accept().await });

    // Accepting fails with EMFILE, but the acceptor keeps trying instead of returning the error.
    time::sleep(Duration::from_millis(500)).await;
    assert!(!accept.is_finished());

    drop(files);
    set_file_limit(limit);

    time::timeout(Duration::from_secs(5), accept)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
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
