use flagset::FlagSet;

use crate::{DeserFromBuf, ReceivedMessage, SerToBuf};

impl<T> DeserFromBuf for FlagSet<T>
where
    T: flagset::Flags<Type = u32>,
{
    fn deserialize(buf: &mut ReceivedMessage<'_>) -> crate::OpenRgbResult<Self> {
        let value = buf.read_value::<u32>()?;
        // TuxRGB patch: OpenRGB keeps adding flag bits (1.0 final added
        // CONTROLLER_FLAG_HIDDEN, MANUALLY_CONFIGURABLE_*, ZONE_FLAG_*…).
        // Unknown bits are ignored instead of failing the whole session.
        Ok(FlagSet::<T>::new_truncated(value))
    }
}

impl<T> SerToBuf for FlagSet<T>
where
    T: flagset::Flags<Type = u32>,
{
    fn serialize(&self, buf: &mut crate::WriteMessage) -> crate::OpenRgbResult<()> {
        buf.write_u32(self.bits());
        Ok(())
    }
}
