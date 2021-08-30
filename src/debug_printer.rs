#[derive(Copy, Clone)]
pub struct DebugPrinter {
    // Used for indentation
    level: usize,
}

impl DebugPrinter {
    pub fn debug<T: std::fmt::Debug>(&self, v: &T) {
        // https://stackoverflow.com/a/42273813/273348
        println!("{:indent$}{:?}", "", v, indent = (2 * self.level));
    }

    pub fn display<T: std::fmt::Display>(&self, v: &T) {
        println!("{:indent$}{}", "", v, indent = (2 * self.level));
    }

    pub fn indent(&self) -> Self {
        Self {
            level: self.level + 1,
        }
    }
}
