use ariel_os::hal::{i2c::controller, peripherals};

ariel_os::hal::group_peripherals!(Peripherals { i2c: I2CPins });

pub type SensorI2c = controller::I2C0;

ariel_os::hal::define_peripherals!(I2CPins {
    i2c_sda: GPIO6,
    i2c_scl: GPIO7,
});
