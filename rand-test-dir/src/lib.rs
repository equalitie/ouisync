use std::{
    collections::{BTreeMap, btree_map},
    fmt,
    io::{BufReader, Read},
    iter,
};

use rand::{
    Rng, RngExt, RngReader, SeedableRng,
    distr::{Alphanumeric, Distribution, SampleString, weighted::WeightedIndex},
    rngs::StdRng,
    seq::IteratorRandom,
};

/// In-memory representation of a directory with its content.
#[derive(Default, Clone, Eq, PartialEq, Debug)]
pub struct Directory {
    pub entries: BTreeMap<String, Entry>,
}

impl Directory {
    /// Generate random directory with its content.
    pub fn generate(rng: &mut dyn Rng, options: GenerateOptions) -> Self {
        let mut dir = Self::default();
        let num_files = rng.random_range(options.min_files..=options.max_files);

        for _ in 0..num_files {
            let mut level = 0;
            let mut current_dir = &mut dir;

            loop {
                let dist = WeightedIndex::new([
                    // put file in the current directory
                    options.max_depth.max(1),
                    // enter existing subdirectory
                    if current_dir
                        .entries
                        .values()
                        .any(|entry| matches!(entry, Entry::Directory(_)))
                    {
                        options.max_depth.max(1)
                    } else {
                        0
                    },
                    // create new directory and enter it
                    options.max_depth.saturating_sub(level),
                ])
                .unwrap();

                match dist.sample(rng) {
                    0 => {
                        let size = rng.random_range(options.min_file_size..=options.max_file_size);
                        let seed = rng.random();
                        let name = current_dir.random_name(rng, &options, 3);

                        current_dir
                            .entries
                            .insert(name, Entry::File(File { size, seed }));

                        break;
                    }
                    1 => {
                        current_dir = current_dir
                            .entries
                            .values_mut()
                            .filter_map(|entry| match entry {
                                Entry::Directory(dir) => Some(dir),
                                Entry::File(_) => None,
                            })
                            .choose(rng)
                            .unwrap();
                        level += 1;
                    }
                    2 => {
                        let name = current_dir.random_name(rng, &options, 0);
                        current_dir = match current_dir
                            .entries
                            .entry(name)
                            .insert_entry(Entry::Directory(Self::default()))
                            .into_mut()
                        {
                            Entry::Directory(dir) => dir,
                            Entry::File(_) => unreachable!(),
                        };
                        level += 1;
                    }
                    _ => unreachable!(),
                }
            }
        }

        dir
    }

    /// Builder-style method to add new entry to this directory.
    pub fn add(mut self, name: String, entry: Entry) -> Self {
        self.entries.insert(name, entry);
        self
    }

    /// Returns an iterator that recursively iterates over all entries of this directory.
    pub fn walk(&self) -> Walk<'_> {
        Walk {
            stack: vec![(self.entries.iter(), "")],
        }
    }

    fn random_name(
        &self,
        rng: &mut dyn Rng,
        options: &GenerateOptions,
        max_extension_len: usize,
    ) -> String {
        let extension = if max_extension_len > 0 {
            let len = rng.random_range(1..=max_extension_len);
            Some(Alphanumeric.sample_string(rng, len).to_ascii_lowercase())
        } else {
            None
        };
        let extension_len = extension.as_ref().map(|e| e.len() + 1).unwrap_or(0); // with dot

        let max_base_len = if options.max_name_len <= extension_len {
            options.max_name_len
        } else {
            options.max_name_len - extension_len
        };

        loop {
            let base_len = rng.random_range(options.min_name_len..=max_base_len);
            let name = Alphanumeric.sample_string(rng, base_len);

            let name = if let Some(extension) = &extension {
                format!("{}.{}", name, extension)
            } else {
                name
            };

            if self.entries.contains_key(&name) {
                continue;
            }

            break name;
        }
    }
}

impl fmt::Display for Directory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (path, entry) in self.walk() {
            let level = path.chars().filter(|c| *c == '/').count();
            let name = path.rsplit_once('/').map(|(_, name)| name).unwrap_or(&path);

            for _ in 0..2 * level {
                write!(f, " ")?;
            }

            match entry {
                Entry::File(File { size, .. }) => {
                    writeln!(f, "📄 {name} ({})", DisplaySize(*size))?
                }
                Entry::Directory(_) => {
                    writeln!(f, "📁 {name}")?;
                }
            }
        }

        Ok(())
    }
}

/// Options for `Directory::generate`
#[derive(Clone, Copy, Debug)]
pub struct GenerateOptions {
    pub min_files: usize,
    pub max_files: usize,
    pub min_file_size: usize,
    pub max_file_size: usize,
    pub max_depth: usize,
    pub min_name_len: usize,
    pub max_name_len: usize,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            min_files: 1,
            max_files: 10,
            min_file_size: 8,
            max_file_size: 1024 * 1024,
            max_depth: 3,
            min_name_len: 3,
            max_name_len: 16,
        }
    }
}

/// Iterator returned by [`Directory::walk`].
pub struct Walk<'a> {
    stack: Vec<(btree_map::Iter<'a, String, Entry>, &'a str)>,
}

impl<'a> Iterator for Walk<'a> {
    type Item = (String, &'a Entry);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (entries, _) = self.stack.last_mut()?;
            match entries.next() {
                Some((name, entry @ Entry::File(_))) => {
                    let path = join(
                        self.stack
                            .iter()
                            .map(|(_, name)| *name)
                            .chain(iter::once(name.as_str())),
                    );
                    return Some((path, entry));
                }
                Some((name, entry @ Entry::Directory(Directory { entries }))) => {
                    self.stack.push((entries.iter(), name));
                    let path = join(self.stack.iter().map(|(_, name)| *name));
                    return Some((path, entry));
                }
                None => {
                    self.stack.pop();
                }
            };
        }
    }
}

fn join<'a>(iter: impl IntoIterator<Item = &'a str>) -> String {
    iter.into_iter().fold(String::new(), |mut m, e| {
        if m.is_empty() {
            m.reserve(e.len());
        } else {
            m.reserve(e.len() + 1);
            m.push('/');
        }

        m.push_str(e);
        m
    })
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct File {
    size: usize,
    seed: u64,
}

impl File {
    /// Obtain a `io::Read` that reads the content of this file.
    pub fn reader(&self) -> impl Read {
        BufReader::new(RngReader(StdRng::seed_from_u64(self.seed)).take(self.size as u64))
    }

    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Entry {
    Directory(Directory),
    File(File),
}

impl From<File> for Entry {
    fn from(value: File) -> Self {
        Self::File(value)
    }
}

impl From<Directory> for Entry {
    fn from(value: Directory) -> Self {
        Self::Directory(value)
    }
}

struct DisplaySize(usize);

impl fmt::Display for DisplaySize {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        const SUFFIXES: &[(usize, &str)] = &[
            (1024, "ki"),
            (1024 * 1024, "Mi"),
            (1024 * 1024 * 1024, "Gi"),
            (1024 * 1024 * 1024 * 1024, "Ti"),
        ];

        for (value, suffix) in SUFFIXES.iter().rev().copied() {
            if self.0 >= value {
                return write!(f, "{:.2} {}B", self.0 as f64 / value as f64, suffix);
            }
        }

        write!(f, "{} B", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader() {
        let mut rng = rand::rng();

        for _ in 0..10 {
            let file = File {
                size: rng.random_range(1..=128 * 1024),
                seed: rng.random(),
            };

            for _ in 0..10 {
                let a = rng.random_range(1..=512);
                let b = rng.random_range(1..=512);

                let content_a = read_all(&file, a);
                let content_b = read_all(&file, b);

                assert_eq!(content_a.len(), content_b.len(), "content lengths differ");

                if let Some(i) = content_a
                    .iter()
                    .zip(content_b.iter())
                    .position(|(a, b)| a != b)
                {
                    panic!("contents differ at offset {i}");
                }
            }
        }
    }

    #[test]
    fn display() {
        let dir = Directory::generate(&mut rand::rng(), GenerateOptions::default());
        println!("{dir}");
    }

    fn read_all(file: &File, chunk_size: usize) -> Vec<u8> {
        let mut output = vec![0; file.size];
        let mut reader = file.reader();
        let mut offset = 0;

        loop {
            let m = chunk_size.min(output.len() - offset);
            let n = reader.read(&mut output[offset..offset + m]).unwrap();
            if n == 0 {
                break;
            }
            offset += n;
        }

        output
    }
}
