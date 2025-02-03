mod utils;

use self::utils::{check_eq, eventually, Bin, CountWrite, RngRead};
use anyhow::{format_err, Result};
use rand::{distributions::Standard, Rng};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    thread,
};

#[test]
#[cfg_attr(any(target_os = "macos", target_os = "ios"), ignore)]
fn transfer_single_small_file() {
    let (a, b) = setup();

    let file_name = "foo.txt";
    let orig_content = "hello";

    fs::write(a.root().join(file_name), orig_content).unwrap();

    eventually(|| {
        let content = fs::read_to_string(b.root().join(file_name))?;
        check_eq(content, orig_content)
    })
}

#[test]
#[cfg_attr(any(target_os = "macos", target_os = "ios"), ignore)]
fn transfer_single_large_file() {
    let (a, b) = setup();

    let file_name = "test.dat";
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
}

#[test]
#[cfg_attr(any(target_os = "macos", target_os = "ios"), ignore)]
fn sequential_write_to_the_same_file() {
    let (a, b) = setup();

    let file_name = "bar.txt";
    let content_a = "hello from A";
    let content_b = "hello from B";

    // A writes first
    fs::write(a.root().join(file_name), content_a).unwrap();

    // B reads what A wrote
    eventually(|| {
        let content = fs::read_to_string(b.root().join(file_name))?;
        check_eq(content, content_a)
    });

    // B writes
    fs::write(b.root().join(file_name), content_b).unwrap();

    // A reads what B wrote
    eventually(|| {
        let content = fs::read_to_string(a.root().join(file_name))?;
        check_eq(content, content_b)
    });
}

#[test]
#[cfg_attr(any(target_os = "macos", target_os = "ios"), ignore)]
fn fast_sequential_writes() {
    // There used to be a deadlock which would manifest whenever one of the connected replicas
    // perfomed more than one write operation (mkdir, echo foo > bar,...) quickly one after another
    // (e.g. "$ mkdir a; mkdir b").
    for _ in 0..5 {
        let (a, b) = setup();
        let count = 10;

        for i in 0..count {
            let file = format!("file-{i}.txt");
            let content = format!("Content of {file}");
            fs::write(a.root().join(file), &content).unwrap();
        }

        eventually(|| check_eq(fs::read_dir(b.root())?.filter(|e| e.is_ok()).count(), count));
    }
}

#[test]
#[cfg_attr(any(target_os = "macos", target_os = "ios"), ignore)]
fn concurrent_read_and_write_small_file() {
    concurrent_read_and_write_file(32);
}

#[test]
#[cfg_attr(any(target_os = "macos", target_os = "ios"), ignore)]
fn concurrent_read_and_write_large_file() {
    concurrent_read_and_write_file(1024 * 1024);
}

fn concurrent_read_and_write_file(size: usize) {
    let (a, b) = setup();
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
#[cfg_attr(any(target_os = "macos", target_os = "ios"), ignore)]
fn concurrent_read_and_delete_file() {
    let (a, b) = setup();

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
        });
        b
    });

    let _a = a_handle.join().unwrap();
    let _b = b_handle.join().unwrap();
}

#[test]
#[cfg_attr(any(target_os = "macos", target_os = "ios"), ignore)]
fn relay() {
    // Create three nodes: A, B and R where A and B are connected only to R but not to each other.
    // Then create a file by A and let it be received by B which requires the file to pass through
    // R first.

    // "relay" node
    let r = Bin::start();
    let r_port = r.bind();
    r.create(None);
    let share_token = r.share();

    let a = Bin::start();
    a.bind();
    a.add_peer(r_port);
    a.create(Some(&share_token));
    a.mount();

    let b = Bin::start();
    b.bind();
    b.add_peer(r_port);
    b.create(Some(&share_token));
    b.mount();

    let file_name = "test.dat";
    let size = 2 * 1024 * 1024;

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
}

#[test]
#[cfg_attr(any(target_os = "macos", target_os = "ios"), ignore)]
fn concurrent_update() {
    let (a, b) = setup();

    let file_name = "test.txt";
    let mut rng = rand::thread_rng();

    // Create a file with initial content by A
    let content_0a: Vec<u8> = (&mut rng).sample_iter(Standard).take(32).collect();
    fs::write(a.root().join(file_name), &content_0a).unwrap();

    // Wait until B sees it
    eventually(|| check_eq(&fs::read(b.root().join(file_name))?, &content_0a));

    // Open the file by both replicas
    let mut file_a = File::options()
        .write(true)
        .open(a.root().join(file_name))
        .unwrap();

    let mut file_b = File::options()
        .write(true)
        .open(b.root().join(file_name))
        .unwrap();

    // Write to it concurrently by both replicas
    let content_1a: Vec<u8> = (&mut rng).sample_iter(Standard).take(64).collect();
    let handle_a = thread::spawn(move || {
        file_a.write_all(&content_1a).unwrap();
        (file_a, content_1a)
    });

    let content_1b: Vec<u8> = (&mut rng).sample_iter(Standard).take(64).collect();
    let handle_b = thread::spawn(move || {
        file_b.write_all(&content_1b).unwrap();
        (file_b, content_1b)
    });

    let (file_a, content_1a) = handle_a.join().unwrap();
    let (file_b, content_1b) = handle_b.join().unwrap();

    drop(file_a);
    drop(file_b);

    // Both replicas see two concurrent versions of the file
    eventually(|| {
        check_concurrent_versions(&a.root().join(file_name), &[&content_1a, &content_1b])
    });
    eventually(|| {
        check_concurrent_versions(&b.root().join(file_name), &[&content_1a, &content_1b])
    });

    // This part of the test currently fails. See https://github.com/equalitie/ouisync/issues/50
    // for more details.

    /*
    let content_2a: Vec<u8> = (&mut rng).sample_iter(Standard).take(64).collect();

    // Update the file again, using the non-disambiguated filename
    let mut file = File::options()
        .write(true)
        .open(a.root().join(file_name))
        .unwrap();
    file.write_all(&content_2a).unwrap();
    drop(file);

    // Both replicas still see two concurrent versions
    eventually(|| {
        check_concurrent_versions(&a.root().join(file_name), &[&content_2a, &content_1b])
    });

    eventually(|| {
        check_concurrent_versions(&b.root().join(file_name), &[&content_2a, &content_1b])
    });

    */
}

fn check_concurrent_versions(file_path: &Path, expected_contents: &[&[u8]]) -> Result<()> {
    let dir = file_path.parent().unwrap();

    // Collect all entries from the directory that start with `file_path`
    let version_paths = fs::read_dir(dir)?
        .map(|entry| Ok(entry?.path()))
        .filter(|entry_path| {
            if let Ok(entry_path) = entry_path {
                // Can't use `Path::starts_with` because that only considers whole path segments.
                entry_path
                    .to_str()
                    .unwrap()
                    .starts_with(file_path.to_str().unwrap())
            } else {
                true
            }
        })
        .collect::<Result<Vec<_>>>()?;

    check_eq(version_paths.len(), expected_contents.len())?;

    // Check that for each item from `expected_contents` there is exactly one file version with
    // that content
    let actual_contents = version_paths
        .into_iter()
        .map(|path: PathBuf| fs::read(path).map_err(Into::into))
        .collect::<Result<HashSet<_>>>()?;

    for expected_content in expected_contents {
        if !actual_contents.contains(*expected_content) {
            return Err(format_err!("expected content missing"));
        }
    }

    Ok(())
}

// This test is similar to the `relay` test but using a "cache server" for the relay node instead
// of a regular peer.
#[test]
#[cfg_attr(any(target_os = "macos", target_os = "ios"), ignore)]
fn mirror() {
    // the cache server
    let r = Bin::start();
    let r_sync_port = r.bind();
    let r_remote_control_port = r.enable_remote_control();

    let a = Bin::start();
    a.bind();
    a.add_peer(r_sync_port);
    a.create(None);
    let share_token = a.share();
    a.mount();

    let b = Bin::start();
    b.bind();
    b.add_peer(r_sync_port);
    b.create(Some(&share_token));
    b.mount();

    a.mirror(&format!("localhost:{r_remote_control_port}"));

    let file_name = "test.dat";
    let size = 1024;

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
}

fn setup() -> (Bin, Bin) {
    let a = Bin::start();
    let a_port = a.bind();
    a.create(None);
    a.mount();
    let share_token = a.share();

    let b = Bin::start();
    b.bind();
    b.add_peer(a_port);
    b.create(Some(&share_token));
    b.mount();

    (a, b)
}
