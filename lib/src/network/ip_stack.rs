use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Protocol {
    Tcp,
    Udp,
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Tcp => write!(f, "TCP"),
            Self::Udp => write!(f, "UDP"),
        }
    }
}
