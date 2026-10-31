//! `luna-store` — sklep z oprogramowaniem środowiska Luna.

use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    luna_store::run()
}
