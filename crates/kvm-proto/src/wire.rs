//! Little-endian cursor helpers used by every encoder and decoder in this crate.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum WireError {
    /// Ran out of input while decoding.
    Truncated,
    /// Output buffer too small while encoding.
    Overflow,
    /// A field holds a value this version does not know.
    Invalid,
}

pub struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn len(&self) -> usize {
        self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.pos == 0
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn bytes(&mut self, data: &[u8]) -> Result<(), WireError> {
        let end = self.pos + data.len();
        self.buf.get_mut(self.pos..end).ok_or(WireError::Overflow)?.copy_from_slice(data);
        self.pos = end;
        Ok(())
    }

    pub fn u8(&mut self, v: u8) -> Result<(), WireError> {
        self.bytes(&[v])
    }

    pub fn u16(&mut self, v: u16) -> Result<(), WireError> {
        self.bytes(&v.to_le_bytes())
    }

    pub fn u32(&mut self, v: u32) -> Result<(), WireError> {
        self.bytes(&v.to_le_bytes())
    }

    pub fn bool(&mut self, v: bool) -> Result<(), WireError> {
        self.u8(v as u8)
    }

    /// A u8 length followed by the bytes.
    pub fn lv(&mut self, data: &[u8]) -> Result<(), WireError> {
        let len = u8::try_from(data.len()).map_err(|_| WireError::Overflow)?;
        self.u8(len)?;
        self.bytes(data)
    }

    pub fn finish(self) -> &'a [u8] {
        &self.buf[..self.pos]
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let end = self.pos.checked_add(n).ok_or(WireError::Truncated)?;
        let s = self.buf.get(self.pos..end).ok_or(WireError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.pos..];
        self.pos = self.buf.len();
        s
    }

    pub fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn bool(&mut self) -> Result<bool, WireError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WireError::Invalid),
        }
    }

    pub fn lv(&mut self) -> Result<&'a [u8], WireError> {
        let n = self.u8()? as usize;
        self.bytes(n)
    }
}

/// `#[repr(u8)]` enums that travel on the wire.
macro_rules! wire_enum {
    ($(#[$m:meta])* pub enum $name:ident { $($(#[$vm:meta])* $var:ident = $val:expr),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "defmt", derive(defmt::Format))]
        #[repr(u8)]
        pub enum $name { $($(#[$vm])* $var = $val),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];

            pub fn from_u8(v: u8) -> Option<Self> {
                match v {
                    $($val => Some($name::$var),)+
                    _ => None,
                }
            }
        }

        impl TryFrom<u8> for $name {
            type Error = $crate::wire::WireError;
            fn try_from(v: u8) -> Result<Self, Self::Error> {
                Self::from_u8(v).ok_or($crate::wire::WireError::Invalid)
            }
        }
    };
}
pub(crate) use wire_enum;
