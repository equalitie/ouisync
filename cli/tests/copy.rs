mod utils;

use self::utils::Bin;
use rand::Rng;
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

fn create_random_file(path: &Path) -> Vec<u8> {
    const TEST_FILE_SIZE: usize = 3 * 65536;
    let mut content = vec![0u8; TEST_FILE_SIZE];
    rand::thread_rng().fill(&mut content[..]);
    let mut file = File::create(path).unwrap();
    file.write_all(&content).unwrap();
    content
}

fn check_file_content(path: &Path, expected_content: &Vec<u8>) {
    let mut file = fs::File::open(path).unwrap();
    let mut content = Vec::new();
    file.read_to_end(&mut content).unwrap();
    assert_eq!(&content, expected_content);
}

#[test]
fn cli_copy_file_to_file() {
    let bin = Bin::start();
    bin.create(None);

    let orig_file_path = bin.aux_dir().join("orig.dat");
    let orig_content = create_random_file(&orig_file_path);

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

    check_file_content(&copy_file_path, &orig_content);
}

#[test]
fn cli_copy_file_to_dir() {
    let bin = Bin::start();
    bin.create(None);

    let orig_file_path = bin.aux_dir().join("orig.dat");
    let orig_content = create_random_file(&orig_file_path);

    let copy_file_path = orig_file_path.parent().unwrap().join("copy.dat");

    let mut cmd = bin.client_command();
    cmd.arg("cp")
        .arg(&orig_file_path)
        .arg("--dst-repo")
        .arg(bin.default_repo_name())
        .arg("/");
    bin.exec(cmd);

    let mut cmd = bin.client_command();
    cmd.arg("cp")
        .arg("--src-repo")
        .arg(bin.default_repo_name())
        .arg(orig_file_path.file_name().unwrap())
        .arg(&copy_file_path);
    bin.exec(cmd);

    check_file_content(&copy_file_path, &orig_content);
}

#[test]
fn cli_copy_dir() {
    let bin = Bin::start();
    bin.create(None);

    let dir1_path = bin.aux_dir().join("dir1");
    let dir2_path = dir1_path.join("dir2");
    fs::create_dir_all(&dir2_path).unwrap();

    let file_path = dir2_path.join("file.dat");
    let file_content = create_random_file(&file_path);

    let mut cmd = bin.client_command();
    cmd.arg("cp")
        .arg(&dir1_path)
        .arg("--dst-repo")
        .arg(bin.default_repo_name())
        .arg("/");
    bin.exec(cmd);

    fs::remove_dir_all(&dir1_path).unwrap();

    let mut cmd = bin.client_command();
    cmd.arg("cp")
        .arg("--src-repo")
        .arg(bin.default_repo_name())
        .arg(dir1_path.file_name().unwrap())
        .arg(dir1_path.parent().unwrap());
    bin.exec(cmd);

    check_file_content(&file_path, &file_content);
}
