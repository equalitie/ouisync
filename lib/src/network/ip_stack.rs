#[derive(Clone)]
pub(super) enum IpStack<T> {
    V4(T),
    V6(T),
    Dual { v4: T, v6: T },
}

impl<T> IpStack<T> {
    pub fn v4(&self) -> Option<&T> {
        match self {
            Self::V4(v4) | Self::Dual { v4, .. } => Some(v4),
            Self::V6(_) => None,
        }
    }

    pub fn v6(&self) -> Option<&T> {
        match self {
            Self::V6(v6) | Self::Dual { v6, .. } => Some(v6),
            Self::V4(_) => None,
        }
    }

    pub fn iter(&self) -> Iter<&T> {
        Iter(Some(self.as_ref()))
    }

    pub fn as_ref(&self) -> IpStack<&T> {
        match self {
            Self::V4(v4) => IpStack::V4(v4),
            Self::V6(v6) => IpStack::V6(v6),
            Self::Dual { v4, v6 } => IpStack::Dual { v4, v6 },
        }
    }

    pub fn try_map<F, U, E>(self, mut f: F) -> Result<IpStack<U>, E>
    where
        F: FnMut(T) -> Result<U, E>,
    {
        match self {
            Self::V4(v4) => Ok(IpStack::V4(f(v4)?)),
            Self::V6(v6) => Ok(IpStack::V6(f(v6)?)),
            Self::Dual { v4, v6 } => Ok(IpStack::Dual {
                v4: f(v4)?,
                v6: f(v6)?,
            }),
        }
    }
}

pub(super) struct Iter<T>(Option<IpStack<T>>);

impl<T> Iterator for Iter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        match self.0.take()? {
            IpStack::Dual { v4, v6 } => {
                self.0 = Some(IpStack::V6(v6));
                Some(v4)
            }
            IpStack::V4(v4) => {
                self.0 = None;
                Some(v4)
            }
            IpStack::V6(v6) => {
                self.0 = None;
                Some(v6)
            }
        }
    }
}
