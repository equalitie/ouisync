mod utils;

use self::utils::Bin;
use rand::Rng;
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
};

fn setup_copy_case() -> (Bin, PathBuf, Vec<u8>) {
    const TEST_FILE_SIZE: usize = 3 * 65536;
    let bin = Bin::start();
    bin.create(None);
    let orig_file_path = bin.aux_dir().join("orig.dat");

    let mut orig_content = vec![0u8; TEST_FILE_SIZE];
    rand::thread_rng().fill(&mut orig_content[..]);
    let mut orig_file = File::create(&orig_file_path).unwrap();
    orig_file.write_all(&orig_content).unwrap();

    (bin, orig_file_path, orig_content)
}

#[test]
fn cli_copy_file_to_file() {
    let (bin, orig_file_path, orig_content) = setup_copy_case();

    let in_repo_file_path = "/in-repo.dat";
    let copy_file_path = orig_file_path.parent().unwrap().join("copy.dat");

    let mut cmd = bin.client_command();
    cmd.arg("cp")
        .arg(&orig_file_path)
        .arg("--dst-repo")
        .arg(bin.default_repo_name())
        .arg(in_repo_file_path);
    bin.exec(cmd);

    let mut cmd = bin.client_command();
    cmd.arg("cp")
        .arg("--src-repo")
        .arg(bin.default_repo_name())
        .arg(in_repo_file_path)
        .arg(&copy_file_path);
    bin.exec(cmd);

    let mut copy_file = fs::File::open(copy_file_path).unwrap();
    let mut copy_content = Vec::new();
    copy_file.read_to_end(&mut copy_content).unwrap();

    assert_eq!(orig_content, copy_content);
}

#[test]
fn cli_copy_file_to_dir() {
    let (a, orig_file_path, orig_content) = setup_copy_case();

    let copy_file_path = orig_file_path.parent().unwrap().join("copy.dat");

    let mut cmd = a.client_command();
    cmd.arg("cp")
        .arg(&orig_file_path)
        .arg("--dst-repo")
        .arg(a.default_repo_name())
        .arg("/");
    a.exec(cmd);

    let mut cmd = a.client_command();
    cmd.arg("cp")
        .arg("--src-repo")
        .arg(a.default_repo_name())
        .arg(orig_file_path.file_name().unwrap())
        .arg(&copy_file_path);
    a.exec(cmd);

    let mut copy_file = fs::File::open(copy_file_path).unwrap();
    let mut copy_content = Vec::new();
    copy_file.read_to_end(&mut copy_content).unwrap();

    assert_eq!(orig_content, copy_content);
}
