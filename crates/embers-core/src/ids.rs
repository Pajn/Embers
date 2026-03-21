use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);

        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                Display::fmt(&self.0, formatter)
            }
        }
    };
}

define_id!(SessionId);
define_id!(BufferId);
define_id!(NodeId);
define_id!(FloatingId);
define_id!(ClientId);
define_id!(RequestId);

#[derive(Debug)]
pub struct IdAllocator<T> {
    next: AtomicU64,
    marker: PhantomData<fn() -> T>,
}

impl<T> IdAllocator<T>
where
    T: From<u64>,
{
    pub const fn new(start_at: u64) -> Self {
        Self {
            next: AtomicU64::new(start_at),
            marker: PhantomData,
        }
    }

    pub fn next(&self) -> T {
        T::from(self.next.fetch_add(1, Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::{IdAllocator, SessionId};

    #[test]
    fn allocator_is_monotonic() {
        let allocator = IdAllocator::<SessionId>::new(41);

        assert_eq!(allocator.next(), SessionId(41));
        assert_eq!(allocator.next(), SessionId(42));
        assert_eq!(allocator.next(), SessionId(43));
    }
}
