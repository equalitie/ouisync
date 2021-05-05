pub const ANSWER: u32 = 42;

#[no_mangle]
pub extern "C" fn hello_world() {
    println!("rust greets dart!")
}
