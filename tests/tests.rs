mod utils;

use self::utils::{eventually, eventually_true, Bin};
use std::{fs, net::Ipv4Addr};

#[test]
fn transfer_single_file() {
    let a = Bin::start(0, []);
    let b = Bin::start(1, [(Ipv4Addr::LOCALHOST, a.port()).into()]);

    let file_name = "foo.txt";
    let orig_content = "hello";

    fs::write(a.root().join(file_name), orig_content).unwrap();

    eventually(|| {
        let content = fs::read_to_string(b.root().join(file_name)).unwrap();
        assert_eq!(content, orig_content);
    })
}

#[test]
fn sequential_write_to_the_same_file() {
    let a = Bin::start(0, []);
    let b = Bin::start(1, [(Ipv4Addr::LOCALHOST, a.port()).into()]);

    let file_name = "bar.txt";
    let content_a = "hello from A";
    let content_b = "hello from B";

    // A writes first
    fs::write(a.root().join(file_name), &content_a).unwrap();

    // B reads what A wrote
    eventually(|| {
        let content = fs::read_to_string(b.root().join(file_name)).unwrap();
        assert_eq!(content, content_a);
    });

    // B writes
    fs::write(b.root().join(file_name), &content_b).unwrap();

    // A reads what B wrote
    eventually(|| {
        let content = fs::read_to_string(a.root().join(file_name)).unwrap();
        assert_eq!(content, content_b);
    });
}

#[test]
fn fast_sequential_writes() {
    // There used to be a deadlock which would manifest whenever one of the connected replicas
    // perfomed more than one write operation (mkdir, echo foo > bar,...) quickly one after another
    // (e.g. "$ mkdir a; mkdir b").
    for _ in 0..5 {
        let a = Bin::start(0, []);
        let b = Bin::start(1, [(Ipv4Addr::LOCALHOST, a.port()).into()]);

        let count = 10;

        for i in 0..count {
            let file = format!("file-{}.txt", i);
            let content = format!("Content of file-{}.txt", i);

            fs::write(a.root().join(file), &content).unwrap();
        }

        // Allowing this lint because even though the `map(|e| e.unwrap())` bit has no effect on the
        // resulting count, it is still useful in that it catches any errors when reading the dir
        // by panicking.
        #[allow(clippy::suspicious_map)]
        eventually_true(|| fs::read_dir(b.root()).unwrap().map(|e| e.unwrap()).count() == count);
    }
}
