use std::io;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
};

/// Check whether the two files have the same content.
pub async fn same_content(lhs: &mut File, rhs: &mut File) -> io::Result<bool> {
    if lhs.metadata().await?.len() != rhs.metadata().await?.len() {
        return Ok(false);
    }

    let mut lhs = BufReader::new(lhs);
    let mut rhs = BufReader::new(rhs);

    loop {
        let lhs_buf = lhs.fill_buf().await?;
        let rhs_buf = rhs.fill_buf().await?;

        match (lhs_buf.is_empty(), rhs_buf.is_empty()) {
            (true, true) => return Ok(true),
            (true, false) | (false, true) => return Ok(false),
            (false, false) => (),
        }

        let n = lhs_buf.len().min(rhs_buf.len());

        if lhs_buf[..n] != rhs_buf[..n] {
            return Ok(false);
        }

        lhs.consume(n);
        rhs.consume(n);
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use tokio::fs;

    use super::*;

    #[tokio::test]
    async fn empty() {
        case(&[], &[], true).await;
    }

    #[tokio::test]
    async fn small_identical() {
        case(b"hello world", b"hello world", true).await;
    }

    #[tokio::test]
    async fn small_different_size() {
        case(b"xxx", b"yyyy", false).await;
    }

    #[tokio::test]
    async fn small_same_size_different_content() {
        case(b"xxxx", b"yyyy", false).await;
    }

    #[tokio::test]
    async fn large_identical() {
        let content = bytes(0, 8 * 1024 * 1024);
        case(&content, &content, true).await;
    }

    #[tokio::test]
    async fn large_different_size() {
        let content_a = bytes(0, 8 * 1024 * 1024);
        let content_b = bytes(0, 8 * 1024 * 1024 + 1);
        case(&content_a, &content_b, false).await;
    }

    #[tokio::test]
    async fn large_same_size_different_content() {
        let content_a = bytes(0, 8 * 1024 * 1024);
        let content_b = bytes(1, 8 * 1024 * 1024);
        case(&content_a, &content_b, false).await;
    }

    #[tokio::test]
    async fn large_same_size_almost_identical_content() {
        let content_a = bytes(0, 8 * 1024 * 1024);
        let content_b = {
            let mut v = bytes(0, 8 * 1024 * 1024);
            *v.last_mut().unwrap() = 0;
            v
        };

        case(&content_a, &content_b, false).await;
    }

    async fn case(lhs_content: &[u8], rhs_content: &[u8], expected: bool) {
        let temp_dir = TempDir::new().unwrap();
        let lhs_path = temp_dir.path().join("lhs");
        let rhs_path = temp_dir.path().join("rhs");

        fs::write(&lhs_path, lhs_content).await.unwrap();
        fs::write(&rhs_path, rhs_content).await.unwrap();

        let mut lhs = File::open(lhs_path).await.unwrap();
        let mut rhs = File::open(rhs_path).await.unwrap();

        let actual = same_content(&mut lhs, &mut rhs).await.unwrap();

        assert_eq!(actual, expected);
    }

    fn bytes(first: u8, count: usize) -> Vec<u8> {
        (first as usize..first as usize + count)
            .map(|n| (n % (u8::MAX as usize + 1)) as u8)
            .collect()
    }
}
