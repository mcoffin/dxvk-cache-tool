use std::fmt::{
    self,
    Display,
};

/// Utility type for displaying an iterator of items separated by an optional separator
pub struct Separated<'a, F> {
    get_iter: F,
    separator: Option<&'a str>,
}

impl<'a, F, It> Separated<'a, F> where
    F: Fn() -> It,
    It: Iterator,
{
    #[inline(always)]
    pub fn new<Sep>(separator: Sep, get_iter: F) -> Self where
        Sep: Into<Option<&'a str>>,
    {
        Separated {
            get_iter: get_iter,
            separator: separator.into(),
        }
    }

    #[inline(always)]
    fn values(&self) -> It {
        (self.get_iter)()
    }
}

impl<'a, F, It> Display for Separated<'a, F> where
    F: Fn() -> It,
    It: Iterator,
    <It as Iterator>::Item: Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut values = self.values();
        if let Some(first) = values.next() {
            write!(f, "{}", first)?;
        }
        let sep = self.separator.unwrap_or("");
        for v in values {
            write!(f, "{}{}", sep, v)?;
        }
        Ok(())
    }
}
