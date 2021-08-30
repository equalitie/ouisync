use async_trait::async_trait;

// https://rust-lang.github.io/async-book/07_workarounds/05_async_in_traits.html
#[async_trait]
pub trait AsyncDebug {
    async fn print(&self, print: &Printer);
}

pub struct Printer {
    // Used for indentation
    level: usize,
}

impl Printer {
    pub fn debug<T: std::fmt::Debug>(&self, v: &T) {
        // https://stackoverflow.com/a/42273813/273348
        println!("{:indent$}{:?}", "", v, indent = (2 * self.level));
    }

    pub fn display<T: std::fmt::Display>(&self, v: &T) {
        println!("{:indent$}{}", "", v, indent = (2 * self.level));
    }

    pub fn indent(&self) -> Printer {
        Printer {
            level: self.level + 1,
        }
    }

    pub async fn print<T: AsyncDebug>(v: &T) {
        let printer = Printer { level: 0 };
        v.print(&printer).await;
    }
}
