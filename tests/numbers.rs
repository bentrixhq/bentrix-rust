//! **A number read off the wire is the number that was written**, to the last bit. serde_json's default float parse
//! takes a fast path that can land one unit in the last place away; a reward, a price or an equity read by this client
//! would then differ from the same figure read by every other language's client, and from the engine's own.

/// Figures the fast path reads one unit in the last place away (the first is a Gym reward the client's own walk met).
#[test]
fn a_number_reads_back_as_it_was_written() {
    for text in ["-0.44868978987099317", "1.1000902", "363.40374086731464", "0.1", "2.2250738585072014e-308"] {
        let read: f64 = serde_json::from_str(text).expect("a number");
        let exact: f64 = text.parse().expect("a number");
        assert_eq!(read.to_bits(), exact.to_bits(), "{text} was read as {read:?}: the JSON parse is not the exact one");
    }
}
