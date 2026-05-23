//! 此文件内容衍生自 Rust 标准库中的 `core::bstr` 模块和 `alloc::bstr` 模块，有修改。
//!
//! （原）版权和许可声明如下：
//! Copyright (c) The Rust Project Contributors
//!
//! Permission is hereby granted, free of charge, to any
//! person obtaining a copy of this software and associated
//! documentation files (the "Software"), to deal in the
//! Software without restriction, including without
//! limitation the rights to use, copy, modify, merge,
//! publish, distribute, sublicense, and/or sell copies of
//! the Software, and to permit persons to whom the Software
//! is furnished to do so, subject to the following
//! conditions:
//!
//! The above copyright notice and this permission notice
//! shall be included in all copies or substantial portions
//! of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
//! ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
//! TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
//! PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
//! SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
//! CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
//! OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
//! IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
//! DEALINGS IN THE SOFTWARE.

use core::{
    borrow::Borrow,
    cmp::Ordering,
    fmt::{self, Alignment, Debug, Display, Write},
    ops::{Deref, DerefPure},
};

use alloc::{borrow::ToOwned, string::String, vec::Vec};

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ByteStr(pub [u8]);

impl ByteStr {
    pub fn new<B: ?Sized + AsRef<[u8]>>(bytes: &B) -> &Self {
        Self::from_bytes(bytes.as_ref())
    }

    pub const fn from_bytes(bytes: &[u8]) -> &Self {
        unsafe { &*(bytes as *const [u8] as *const Self) }
    }

    pub const fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Deref for ByteStr {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

unsafe impl DerefPure for ByteStr {}

impl Debug for ByteStr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "\"")?;
        for chunk in self.utf8_chunks() {
            for c in chunk.valid().chars() {
                match c {
                    '\0' => write!(f, "\\0")?,
                    '\x01'..='\x7f' => write!(f, "{}", (c as u8).escape_ascii())?,
                    _ => write!(f, "{}", c.escape_debug())?,
                }
            }
            write!(f, "{}", chunk.invalid().escape_ascii())?;
        }
        write!(f, "\"")?;
        Ok(())
    }
}

impl Display for ByteStr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        fn emit(byte_str: &ByteStr, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for chunk in byte_str.utf8_chunks() {
                f.write_str(chunk.valid())?;
                if !chunk.invalid().is_empty() {
                    f.write_str("\u{FFFD}")?;
                }
            }

            Ok(())
        }

        let requested_width = f.width().unwrap_or(0);
        if requested_width == 0 && f.precision().is_none() {
            // Avoid counting the characters if no truncation or padding was
            // requested.
            return emit(self, f);
        }

        let (truncated, actual_width): (&[u8], _) = match f.precision() {
            // The entire string is truncated away. Weird, but ok.
            Some(0) => (&[], 0),
            // Advance through string until we run out of space.
            Some(precision) => {
                let mut remaining_width = precision;
                let mut chunks = self.utf8_chunks();
                let mut current_width = 0;
                let mut offset = 0;
                loop {
                    let Some(chunk) = chunks.next() else {
                        // We reached the end of the string without running out
                        // of space, so print the entire string.
                        break (self, current_width);
                    };

                    let mut chars = chunk.valid().char_indices();
                    let Err(remaining) = chars.advance_by(remaining_width) else {
                        // We've counted off `precision` characters, so truncate
                        // the string at the current offset.
                        break (&self[..offset + chars.offset()], precision);
                    };

                    offset += chunk.valid().len();
                    current_width += remaining_width - remaining.get();
                    remaining_width = remaining.get();

                    // `remaining_width` cannot be zero, there is still space
                    // remaining. So next, count the � character emitted for
                    // the invalid chunk (if it exists).
                    if !chunk.invalid().is_empty() {
                        offset += chunk.invalid().len();
                        current_width += 1;
                        remaining_width -= 1;

                        if remaining_width == 0 {
                            break (&self[..offset], precision);
                        }
                    }
                }
            }
            // The string shouldn't be truncated at all, so just count the number
            // of characters to calculate the padding.
            None => {
                let actual_width = self
                    .utf8_chunks()
                    .map(|chunk| {
                        chunk.valid().chars().count()
                            + if chunk.invalid().is_empty() { 0 } else { 1 }
                    })
                    .sum();
                (self, actual_width)
            }
        };

        // The width is originally stored as a 16-bit number, so this cannot fail.
        let padding = u16::try_from(requested_width.saturating_sub(actual_width)).unwrap();

        let align = f.align().unwrap_or(Alignment::Left);
        let fill = f.fill();

        let padding_left = match align {
            Alignment::Left => 0,
            Alignment::Right => padding,
            Alignment::Center => padding / 2,
        };

        for _ in 0..padding_left {
            f.write_char(fill)?;
        }

        let padding = padding - padding_left;

        emit(&ByteStr::new(truncated), f)?;
        for _ in 0..padding {
            f.write_char(fill)?;
        }
        Ok(())
    }
}

impl AsRef<[u8]> for ByteStr {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl AsRef<ByteStr> for ByteStr {
    fn as_ref(&self) -> &ByteStr {
        self
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub struct ByteString(pub Vec<u8>);

impl ToOwned for ByteStr {
    type Owned = ByteString;

    fn to_owned(&self) -> Self::Owned {
        ByteString(self.0.to_vec())
    }
}

impl Deref for ByteString {
    type Target = ByteStr;

    fn deref(&self) -> &Self::Target {
        let ptr: *const [u8] = self.0.as_slice();
        unsafe { &*(ptr as *const ByteStr) }
    }
}

macro_rules! impl_partial_eq {
    ($lhs:ty, $rhs:ty) => {
        impl PartialEq<$rhs> for $lhs {
            #[inline]
            fn eq(&self, other: &$rhs) -> bool {
                let other: &[u8] = other.as_ref();
                PartialEq::eq(self.as_bytes(), other)
            }
        }

        impl PartialEq<$lhs> for $rhs {
            #[inline]
            fn eq(&self, other: &$lhs) -> bool {
                let this: &[u8] = self.as_ref();
                PartialEq::eq(this, other.as_bytes())
            }
        }
    };
}

macro_rules! impl_partial_eq_n {
    ($lhs:ty, $rhs:ty) => {
        impl<const N: usize> PartialEq<$rhs> for $lhs {
            #[inline]
            fn eq(&self, other: &$rhs) -> bool {
                let other: &[u8] = other.as_ref();
                PartialEq::eq(self.as_bytes(), other)
            }
        }

        impl<const N: usize> PartialEq<$lhs> for $rhs {
            #[inline]
            fn eq(&self, other: &$lhs) -> bool {
                let this: &[u8] = self.as_ref();
                PartialEq::eq(this, other.as_bytes())
            }
        }
    };
}

impl_partial_eq!(ByteStr, [u8]);
impl_partial_eq!(ByteStr, &[u8]);
impl_partial_eq!(ByteStr, str);
impl_partial_eq!(ByteStr, &str);
impl_partial_eq_n!(ByteStr, [u8; N]);
impl_partial_eq_n!(ByteStr, &[u8; N]);

unsafe impl DerefPure for ByteString {}

impl Debug for ByteString {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Debug::fmt(self.deref(), f)
    }
}

impl Display for ByteString {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Display::fmt(self.deref(), f)
    }
}

impl AsRef<[u8]> for ByteString {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}
impl AsRef<ByteStr> for ByteString {
    fn as_ref(&self) -> &ByteStr {
        self.deref()
    }
}

impl Borrow<ByteStr> for ByteString {
    fn borrow(&self) -> &ByteStr {
        self.deref()
    }
}

macro_rules! impl_partial_eq_ord {
    ($lhs:ty, $rhs:ty) => {
        impl_partial_eq!($lhs, $rhs);

        impl PartialOrd<$rhs> for $lhs {
            #[inline]
            fn partial_cmp(&self, other: &$rhs) -> Option<Ordering> {
                let other: &[u8] = other.as_ref();
                PartialOrd::partial_cmp(self.as_bytes(), other)
            }
        }

        impl PartialOrd<$lhs> for $rhs {
            #[inline]
            fn partial_cmp(&self, other: &$lhs) -> Option<Ordering> {
                let this: &[u8] = self.as_ref();
                PartialOrd::partial_cmp(this, other.as_bytes())
            }
        }
    };
}
impl_partial_eq!(ByteString, Vec<u8>);
impl_partial_eq!(ByteString, [u8]);
impl_partial_eq!(ByteString, &[u8]);
impl_partial_eq!(ByteString, String);
impl_partial_eq!(ByteString, str);
impl_partial_eq!(ByteString, &str);
impl_partial_eq_ord!(ByteString, ByteStr);
impl_partial_eq_ord!(ByteString, &ByteStr);
impl_partial_eq_n!(ByteString, [u8; N]);
impl_partial_eq_n!(ByteString, &[u8; N]);
