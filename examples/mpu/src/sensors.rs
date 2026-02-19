//! This module is intended to contain the auto-@generated instantiation and registration of sensor
//! drivers.

pub async fn init() {
    #[cfg(context = "esp32c6")]
    mpu6050::init().await;
    info!("MPU6050 sensor initialized");
}

mod mpu6050 {
    use ariel_os::i2c::controller::I2cDevice;

    use ariel_os_sensor_mpu6050::i2c::{Mpu6050Sensor, Peripherals};

    pub static MPU6050_I2C: Mpu6050Sensor<I2cDevice<'_>> =
        const { Mpu6050Sensor::new(Some("onboard")) };

    #[ariel_os::reexports::linkme::distributed_slice(ariel_os::sensors::SENSOR_REFS)]
    #[linkme(crate = ariel_os::reexports::linkme)]
    static MPU6050_I2C_REF: &'static dyn ariel_os::sensors::Sensor = &MPU6050_I2C;

    #[ariel_os::task(autostart)]
    pub async fn mpu6050_runner() {
        MPU6050_I2C.run().await
    }

    pub(super) async fn init() {
        let bus = crate::i2c_bus::I2C_BUS.get().unwrap();

        MPU6050_I2C.init(Peripherals {}, I2cDevice::new(bus)).await;
    }
}

use ariel_os::debug::log::info;
#[allow(
    unused,
    reason = "should be directly accessible without going through the registry"
)]
#[cfg(context = "esp32c6")]
pub use mpu6050::MPU6050_I2C;
