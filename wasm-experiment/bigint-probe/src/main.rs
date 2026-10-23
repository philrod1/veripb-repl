use malachite_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};

fn main() {
    let vals: [i128; 12] = [
        0, 1, -1, 99_999_999_999_999, 100_000_000_000_000, 500_000_000_000_000,
        999_999_999_999_999, 8_999_999_999_999_997, i64::MAX as i128, i64::MIN as i128,
        i64::MAX as i128 + 1, -(i64::MAX as i128) - 2,
    ];
    let mut big = Vec::new();
    for &a in &vals {
        for &b in &vals {
            let p = a.checked_mul(b);
            let s = a.checked_add(b);
            let ba = BigInt::from(a);
            let bb = BigInt::from(b);
            let bp = &ba * &bb;
            let bs = &ba + &bb;
            let rt_p: Option<i128> = (&bp).try_into().ok();
            let rt_s: Option<i128> = (&bs).try_into().ok();
            let rt_p64: Option<i64> = (&bp).try_into().ok();
            let div = if b != 0 { Some((a.div_euclid(b), (&ba / &bb).to_string(), (&ba % &bb).to_string())) } else { None };
            println!("{a} {b} | {p:?} {s:?} | {bp} {bs} | {rt_p:?} {rt_s:?} {rt_p64:?} | {div:?} | {} {}", bp.abs(), bp.is_zero());
            big.push(bp);
        }
    }
    // larger values, beyond i128
    let mut acc = BigInt::from(1);
    for _ in 0..6 { acc = &acc * BigInt::from(500_000_000_000_000i64); println!("pow {acc} {:?} {:?}", acc.to_i128(), TryInto::<i128>::try_into(&acc).ok()); }
    let sum: BigInt = big.iter().fold(BigInt::from(0), |x, y| x + y);
    println!("sum {sum}");
}
