mod utils;

use anyhow::format_err;

use self::utils::{check_eq, eventually, Bin, CountWrite, RngRead};
use std::{
    fs::{self, File},
    io::{self, Read},
    net::Ipv4Addr,
    thread,
};

#[test]
fn transfer_single_file() {
    let a = Bin::start(0, [], None);
    let b = Bin::start(
        1,
        [(Ipv4Addr::LOCALHOST, a.port()).into()],
        Some(a.share_token()),
    );

    let file_name = "foo.txt";
    let orig_content = "hello";

    fs::write(a.root().join(file_name), orig_content).unwrap();

    eventually(|| {
        let content = fs::read_to_string(b.root().join(file_name))?;
        check_eq(content, orig_content)
    })
}

#[test]
fn sequential_write_to_the_same_file() {
    let a = Bin::start(0, [], None);
    let b = Bin::start(
        1,
        [(Ipv4Addr::LOCALHOST, a.port()).into()],
        Some(a.share_token()),
    );

    let file_name = "bar.txt";
    let content_a = "hello from A";
    let content_b = "hello from B";

    // A writes first
    fs::write(a.root().join(file_name), &content_a).unwrap();

    // B reads what A wrote
    eventually(|| {
        let content = fs::read_to_string(b.root().join(file_name))?;
        check_eq(content, content_a)
    });

    // B writes
    fs::write(b.root().join(file_name), &content_b).unwrap();

    // A reads what B wrote
    eventually(|| {
        let content = fs::read_to_string(a.root().join(file_name))?;
        check_eq(content, content_b)
    });
}

#[test]
fn fast_sequential_writes() {
    // There used to be a deadlock which would manifest whenever one of the connected replicas
    // perfomed more than one write operation (mkdir, echo foo > bar,...) quickly one after another
    // (e.g. "$ mkdir a; mkdir b").
    for _ in 0..5 {
        let a = Bin::start(0, [], None);
        let b = Bin::start(
            1,
            [(Ipv4Addr::LOCALHOST, a.port()).into()],
            Some(a.share_token()),
        );

        let count = 10;

        for i in 0..count {
            let file = format!("file-{}.txt", i);
            let content = format!("Content of {}", file);
            fs::write(a.root().join(file), &content).unwrap();
        }

        eventually(|| check_eq(fs::read_dir(b.root())?.filter(|e| e.is_ok()).count(), count));
    }
}

#[test]
fn concurrent_read_and_write_small_file() {
    concurrent_read_and_write_file(32);
}

#[test]
fn concurrent_read_and_write_large_file() {
    concurrent_read_and_write_file(1024 * 1024);
}

fn concurrent_read_and_write_file(size: usize) {
    let a = Bin::start(0, [], None);
    let b = Bin::start(
        1,
        [(Ipv4Addr::LOCALHOST, a.port()).into()],
        Some(a.share_token()),
    );

    let file_name = "test.txt";

    let a_handle = thread::spawn(move || {
        let mut src = RngRead(rand::thread_rng()).take(size as u64);
        let mut dst = File::create(a.root().join(file_name)).unwrap();
        io::copy(&mut src, &mut dst).unwrap();

        a
    });

    let b_handle = thread::spawn(move || {
        eventually(|| {
            let mut src = File::open(b.root().join(file_name))?;
            let mut dst = CountWrite(0);
            io::copy(&mut src, &mut dst)?;

            check_eq(dst.0, size)
        });

        b
    });

    let _a = a_handle.join().unwrap();
    let _b = b_handle.join().unwrap();

    // TODO: check the files are identical
}

// This used to cause deadlock. For the deadlock to be triggered, the file to be deleted must be
// large enough so that the number of blocks it consists of is greater than the capacity of the
// notification channel.
#[test]
fn concurrent_read_and_delete_file() {
    let a = Bin::start(0, [], None);
    let b = Bin::start(
        1,
        [(Ipv4Addr::LOCALHOST, a.port()).into()],
        Some(a.share_token()),
    );

    let file_name = "test.txt";
    let size = 4 * 1024 * 1024;

    // Create the file by A
    {
        let mut src = RngRead(rand::thread_rng()).take(size as u64);
        let mut dst = File::create(a.root().join(file_name)).unwrap();
        io::copy(&mut src, &mut dst).unwrap();
    }

    // Wait until it's fully received by B
    eventually(|| {
        let mut src = File::open(b.root().join(file_name))?;
        let mut dst = CountWrite(0);
        io::copy(&mut src, &mut dst)?;

        check_eq(dst.0, size)
    });

    // Delete the file by A and concurrently read it by B
    let a_handle = thread::spawn(move || {
        fs::remove_file(a.root().join(file_name)).unwrap();
        a
    });

    let b_handle = thread::spawn(move || {
        eventually(|| match fs::metadata(b.root().join(file_name)) {
            Ok(_) => Err(format_err!("file should not exist: '{}'", file_name)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        })
    });

    let _a = a_handle.join().unwrap();
    let _b = b_handle.join().unwrap();
}
