use embassy_rp::i2c::{Async, I2c};
use embassy_rp::peripherals::I2C0;

const REG_TEMP: u8 = 0xFA;
const REG_CALIB: u8 = 0x88;

pub struct BMP280 {
    bus: I2c<'static, I2C0, Async>,
    address: u8,
    dig_t1: u16,
    dig_t2: i16,
    dig_t3: i16,
}

impl BMP280 {
    /// Initilize new and read calibration
    pub async fn new(mut bus: I2c<'static, I2C0, Async>, addr: u8) -> Self {
        let mut buf = [0u8; 6];
        bus.write_read_async(addr, REG_CALIB.to_be_bytes(), &mut buf)
            .await
            .unwrap();

        Self {
            bus: bus,
            address: addr,
            dig_t1: ((buf[1] as u16) << 8) | buf[0] as u16,
            dig_t2: ((buf[3] as i16) << 8) | buf[2] as i16,
            dig_t3: ((buf[5] as i16) << 8) | buf[4] as i16,
        }
    }
    pub async fn read_temp(&mut self) -> f32 {
        let mut buf = [0u8; 3];
        self.bus
            .write_read_async(self.address, REG_TEMP.to_be_bytes(), &mut buf)
            .await
            .unwrap();

        let adc_t =
            (((buf[0] as u32) << 12) | ((buf[1] as u32) << 4) | ((buf[2] as u32) >> 4)) as i32;
        let var1 = (((adc_t >> 3) - ((self.dig_t1 as i32) << 1)) * self.dig_t2 as i32) >> 11;
        let var2 = (((((adc_t >> 4) - self.dig_t1 as i32) * ((adc_t >> 4) - self.dig_t1 as i32))
            >> 12)
            * self.dig_t3 as i32)
            >> 14;
        let t_fine = var1 + var2;
        (((t_fine * 5 + 128) >> 8) as f32) / 100.0
    }
}
