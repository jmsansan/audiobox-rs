//! Private error bridge for the adapted packet DSP.
#[derive(Debug)]
pub(super) enum CodecError {
    PacketEnd,
    InvalidFormat(String),
    CorruptData(String),
    UnsupportedFeature(String),
    BufferTooSmall { needed: usize, provided: usize },
    LimitExceeded(&'static str),
}
impl From<CodecError> for crate::Error {
    fn from(error: CodecError) -> Self {
        use CodecError::*;
        match error {
            PacketEnd => Self::decode("Unexpected end of codec header or packet"),
            InvalidFormat(s) | CorruptData(s) => Self::decode(s),
            UnsupportedFeature(s) => Self::new(crate::ErrorCode::UnsupportedFormat, s),
            BufferTooSmall { needed, provided } => {
                Self::invalid(format!("Codec needs {needed} samples, got {provided}"))
            }
            LimitExceeded(s) => Self::limit(s),
        }
    }
}
/// Conservative preparation accounting, separate from delivered PCM. Charge
/// before every data-dependent codebook allocation, including temporary tables.
pub(super) struct Budget {
    pub remaining: usize,
}
impl Budget {
    pub fn charge(&mut self, bytes: usize) -> Result<(), CodecError> {
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or(CodecError::LimitExceeded(
                "Ogg codec workspace exceeds byte limit",
            ))?;
        Ok(())
    }
}
