use std::{fmt, str::FromStr};

use data_encoding::{BASE32_NOPAD_NOCASE, HEXLOWER_PERMISSIVE};
use p2panda::spaces::Member;

use crate::Error;

/// A verified device membership bundle. Its display form is unpadded base32.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberInfo([u8; 305]);

impl MemberInfo {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let info = Self(bytes.try_into().map_err(invalid)?);
        info.member()?;
        Ok(info)
    }

    pub fn as_bytes(&self) -> &[u8; 305] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        HEXLOWER_PERMISSIVE.encode(&self.0)
    }

    pub fn from_hex(text: &str) -> Result<Self, Error> {
        Self::from_bytes(
            &HEXLOWER_PERMISSIVE
                .decode(text.as_bytes())
                .map_err(invalid)?,
        )
    }

    pub fn to_base32(&self) -> String {
        BASE32_NOPAD_NOCASE.encode(&self.0)
    }

    pub fn from_base32(text: &str) -> Result<Self, Error> {
        Self::from_bytes(
            &BASE32_NOPAD_NOCASE
                .decode(text.as_bytes())
                .map_err(invalid)?,
        )
    }

    pub(crate) fn member(&self) -> Result<Member, Error> {
        let member = Member::from_bytes(&self.0).map_err(invalid)?;
        member.verify().map_err(invalid)?;
        Ok(member)
    }
}

impl fmt::Display for MemberInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base32())
    }
}

impl FromStr for MemberInfo {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.len() {
            len if len == 305 * 2 => Self::from_hex(text),
            _ => Self::from_base32(text),
        }
    }
}

fn invalid(error: impl fmt::Display) -> Error {
    Error::Member(error.to_string())
}
