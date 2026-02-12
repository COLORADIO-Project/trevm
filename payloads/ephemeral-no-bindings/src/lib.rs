#![no_std]

#[global_allocator]
static ALLOCATOR: talc::Talck<talc::locking::AssumeUnlockable, talc::ClaimOnOom> = {
    static mut MEMORY: [u8; 0x4000] = [0; 0x4000]; // 16KiB of memory
    let span = talc::Span::from_array((&raw const MEMORY).cast_mut());
    talc::Talc::new(unsafe { talc::ClaimOnOom::new(span) }).lock()
};

use wit_bindgen::generate;

extern crate alloc;
generate!({
    world: "example-ephemeral-no-bindings ",
    path: "../../wit",
    generate_all,
});

pub struct MyComponent;

impl Guest for MyComponent {
    fn fibonacci(n: u32) -> u32 {
        if n == 0 { return 0; }
        else if n == 1 { return 1; }
        let mut f_0 = 0;
        let mut f_1 = 1;
        for _ in 2..=n {
            f_1 += f_0;
            f_0 = f_1 - f_0;
        }
        return f_1
    }
}

export!(MyComponent);

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable();
}