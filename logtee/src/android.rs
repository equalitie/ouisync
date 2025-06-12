// Implementation based on piping logcat output to the log file.

use std::{
    io::{self, Read, Write},
    panic,
    path::Path,
    process::{self, Child, Command},
    thread::{self, JoinHandle},
};

use super::{create_file, RotateOptions};

pub(super) struct Inner {
    process: Child,
    handle: JoinHandle<io::Result<()>>,
}

impl Inner {
    pub fn new(path: &Path, options: RotateOptions) -> io::Result<Self> {
        let mut file = create_file(path, options);
        let (mut pipe_reader, pipe_writer) = io::pipe()?;

        let handle = thread::spawn(move || {
            let mut buffer = vec![0; 1024];

            loop {
                let n = pipe_reader.read(&mut buffer)?;
                if n > 0 {
                    file.write_all(&buffer[..n])?;
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
