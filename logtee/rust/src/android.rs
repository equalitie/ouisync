// Implementation based on piping logcat output to the log file.

use std::{
    io::{self, BufRead, BufReader, PipeReader, Write},
    panic,
    process::{self, Child, Command},
    thread::{self, JoinHandle},
};

use chrono::{DateTime, Utc};
use file_rotate::{suffix::AppendCount, FileRotate};

pub(super) struct Inner {
    process: Child,
    handle: JoinHandle<io::Result<()>>,
}

impl Inner {
    pub fn new(mut file: FileRotate<AppendCount>) -> io::Result<Self> {
        let (pipe_reader, pipe_writer) = io::pipe()?;

        let handle = thread::spawn(move || {
            let mut pipe_reader = BufReader::new(pipe_reader);

            let line = skip_irrelevant_lines(&mut pipe_reader)?;
            file.write_all(line.as_bytes())?;
            drop(line);

            loop {
                let chunk = pipe_reader.fill_buf()?;
                let n = chunk.len();

                if n > 0 {
                    file.write_all(chunk)?;
                    pipe_reader.consume(n);
                } else {
                    return Ok(());
                }
            }
        });

        let process = Command::new("logcat")
            .arg("--pid")
            .arg(process::id().to_string())
            .arg("-vtime")
            .arg("-vyear")
            .arg("-vUTC")
            .stdout(pipe_writer)
            .spawn()?;

        Ok(Self { process, handle })
    }

    pub fn close(mut self) -> io::Result<()> {
        self.process.kill()?;
        self.process.wait()?;

        match self.handle.join() {
            Ok(result) => result,
            Err(payload) => panic::resume_unwind(payload),
        }
    }
}

// Skip lines that don't start with a timestamp (e.g., a buffer header) or whose timestamp is in the
// past. Returns the first relevant line.
fn skip_irrelevant_lines(reader: &mut BufReader<PipeReader>) -> io::Result<String> {
    let now = Utc::now();
    let mut line = String::new();

    loop {
        line.clear();
        reader.read_line(&mut line)?;

        if line.is_empty() {
            return Ok(line);
        }

        match DateTime::parse_and_remainder(&line, "%Y-%m-%d %H:%M:%S%.f %z") {
            Ok((timestamp, _)) if timestamp >= now => return Ok(line),
            Ok(_) | Err(_) => continue,
        }
    }
}
