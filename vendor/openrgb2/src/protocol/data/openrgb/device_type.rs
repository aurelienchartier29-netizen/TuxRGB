/// RGB controller device type.
///
/// See [Open SDK documentation](https://gitlab.com/CalcProgrammer1/OpenRGB/-/wikis/OpenRGB-SDK-Documentation) for more information.
#[derive(Eq, PartialEq, Debug, Copy, Clone, Hash)]
pub enum DeviceType {
    /// Motherboard.
    Motherboard = 0,
    /// DRAM
    DRam = 1,
    /// GPU
    Gpu = 2,
    /// Cooler
    Cooler = 3,
    /// LED strip
    LEDStrip = 4,
    /// Keyboard
    Keyboard = 5,
    /// Mouse
    Mouse = 6,
    /// Mouse mat
    MouseMat = 7,
    /// Headset
    Headset = 8,
    /// Headset stand
    HeadsetStand = 9,
    /// Gamepad
    Gamepad = 10,
    /// Light
    Light = 11,
    /// Speaker
    Speaker = 12,
    /// Virtual
    Virtual = 13,
    /// Storage (OpenRGB 1.0)
    Storage = 14,
    /// Case (OpenRGB 1.0)
    Case = 15,
    /// Microphone (OpenRGB 1.0)
    Microphone = 16,
    /// Accessory (OpenRGB 1.0)
    Accessory = 17,
    /// Keypad (OpenRGB 1.0)
    Keypad = 18,
    /// Laptop (OpenRGB 1.0)
    Laptop = 19,
    /// Monitor (OpenRGB 1.0)
    Monitor = 20,
    /// Unknown (always the last value in OpenRGB, 21 since 1.0)
    Unknown = 21,
}

// TuxRGB patch: OpenRGB 1.0 final types (`DEVICE_TYPE_STORAGE` = 14 …
// `DEVICE_TYPE_UNKNOWN` = 21). Unknown future values map to `Unknown`
// instead of aborting the controller list.
impl TryFrom<u32> for DeviceType {
    type Error = crate::OpenRgbError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Motherboard,
            1 => Self::DRam,
            2 => Self::Gpu,
            3 => Self::Cooler,
            4 => Self::LEDStrip,
            5 => Self::Keyboard,
            6 => Self::Mouse,
            7 => Self::MouseMat,
            8 => Self::Headset,
            9 => Self::HeadsetStand,
            10 => Self::Gamepad,
            11 => Self::Light,
            12 => Self::Speaker,
            13 => Self::Virtual,
            14 => Self::Storage,
            15 => Self::Case,
            16 => Self::Microphone,
            17 => Self::Accessory,
            18 => Self::Keypad,
            19 => Self::Laptop,
            20 => Self::Monitor,
            _ => Self::Unknown,
        })
    }
}

impl From<DeviceType> for u32 {
    fn from(value: DeviceType) -> Self {
        value as u32
    }
}

impl<'a> From<&'a DeviceType> for u32 {
    fn from(value: &'a DeviceType) -> Self {
        *value as u32
    }
}

impl crate::protocol::DeserFromBuf for DeviceType {
    fn deserialize(buf: &mut crate::protocol::ReceivedMessage<'_>) -> crate::OpenRgbResult<Self> {
        let raw = buf.read_value::<u32>()?;
        DeviceType::try_from(raw)
    }
}

impl crate::protocol::SerToBuf for DeviceType {
    fn serialize(&self, buf: &mut crate::protocol::WriteMessage) -> crate::OpenRgbResult<()> {
        buf.write_u32(u32::from(self));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use crate::{WriteMessage, data::DeviceType};

    #[tokio::test]
    async fn test_read_001() -> Result<(), Box<dyn Error>> {
        let mut buf = WriteMessage::new(crate::DEFAULT_PROTOCOL);
        let mut msg = buf.push_value(3_u32)?.to_received_msg();

        assert_eq!(msg.read_value::<DeviceType>()?, DeviceType::Cooler);
        Ok(())
    }

    #[tokio::test]
    async fn test_write_001() -> Result<(), Box<dyn Error>> {
        let mut buf = WriteMessage::new(crate::DEFAULT_PROTOCOL);
        let mut msg = buf.push_value(DeviceType::Cooler)?.to_received_msg();
        assert_eq!(msg.read_value::<u32>()?, 3);
        Ok(())
    }
}
