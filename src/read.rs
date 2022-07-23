use std::io::Read;

pub trait FromReader: Sized {
    type Error: Sized;
    fn from_reader<R>(r: R) -> Result<Self, Self::Error>
    where
        R: Read;
}
