//! `random(seed)`: a generator whose draws are fixed by its seed, so a run stays
//! deterministic (`command-mode.md` §8) while scattering, jittering or picking.
//! It is xoshiro256** seeded through SplitMix64, written here so the numbers a
//! seed draws never change with a dependency's version.

use mlua::{Lua, MetaMethod, MultiValue, UserData, UserDataMethods, Value};

/// One generator, `random(seed)` in a script.
pub(crate) struct Rng {
    state: [u64; 4],
    /// The seed as the script wrote it, for `tostring`.
    shown: String,
}

/// SplitMix64's next value from `x`, advancing it: how one 64-bit seed becomes
/// the four words xoshiro needs.
fn split_mix(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Rng {
    /// A generator seeded with `seed`.
    pub(crate) fn new(seed: u64, shown: String) -> Self {
        let mut x = seed;
        let state = [
            split_mix(&mut x),
            split_mix(&mut x),
            split_mix(&mut x),
            split_mix(&mut x),
        ];
        Self { state, shown }
    }

    /// The next 64 random bits.
    fn next_u64(&mut self) -> u64 {
        let s = &mut self.state;
        let out = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        out
    }

    /// A number from 0 up to but not including 1, from the top 53 bits.
    pub(crate) fn fraction(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A whole number from `lo` to `hi`, both included; `lo <= hi`.
    pub(crate) fn whole(&mut self, lo: i64, hi: i64) -> i64 {
        let span = i128::from(hi) - i128::from(lo) + 1;
        // Every 64-bit value is in range: the draw is the bits themselves.
        let Ok(span) = u64::try_from(span) else {
            return self.next_u64().cast_signed();
        };
        // Lemire's multiply-shift, rejecting the few draws that would bias it.
        let threshold = span.wrapping_neg() % span;
        loop {
            let m = u128::from(self.next_u64()) * u128::from(span);
            if (m as u64) >= threshold {
                // Below `span`, so `lo` plus it is at most `hi`.
                return (i128::from(lo) + (m >> 64).cast_signed()) as i64;
            }
        }
    }
}

/// A Luau integer as 64 bits: mlua's `Integer` is 32 bits on wasm32, where the
/// exported page runs scripts, and 64 on the desktop.
#[allow(
    clippy::useless_conversion,
    reason = "the conversion is a no-op only where `Integer` is i64"
)]
fn wide(i: mlua::Integer) -> i64 {
    i64::from(i)
}

/// The seed `v` as 64 bits: a whole number as itself, so `random(1)` and
/// `random(1.0)` agree; another number by its bits; a string by its FNV-1a hash.
pub(crate) fn seed_of(v: &Value) -> Option<(u64, String)> {
    match v {
        Value::Integer(i) => Some((wide(*i).cast_unsigned(), i.to_string())),
        Value::Number(n) if n.is_finite() => {
            let shown = crate::format_number(*n);
            if n.fract() == 0.0 && n.abs() < 9.0e15 {
                Some(((*n as i64).cast_unsigned(), shown))
            } else {
                Some((n.to_bits(), shown))
            }
        }
        Value::String(s) => {
            let bytes = s.as_bytes();
            let mut h: u64 = 0xCBF2_9CE4_8422_2325;
            for b in bytes.iter() {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01B3);
            }
            Some((h, format!("\"{}\"", s.to_string_lossy())))
        }
        _ => None,
    }
}

/// `v` as a whole number for a draw's bound, or why not.
fn bound(v: &Value) -> Result<i64, String> {
    let n = match v {
        Value::Integer(i) => return Ok(wide(*i)),
        Value::Number(n) => *n,
        _ => return Err("takes whole numbers".into()),
    };
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.0e15 {
        Ok(n as i64)
    } else {
        Err(format!(
            "takes whole numbers, not {} — rng() * {} scales a fraction",
            crate::format_number(n),
            crate::format_number(n)
        ))
    }
}

impl UserData for Rng {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // `rng()` a fraction, `rng(n)` a whole number from 1 to n, `rng(m, n)` one
        // from m to n: `math.random`'s three forms.
        methods.add_meta_method_mut(MetaMethod::Call, |lua, this, args: MultiValue| {
            let args: Vec<Value> = args.into_iter().collect();
            let fail = |why: String| crate::run::raise(lua, format!("A random draw {why}"));
            let (lo, hi) = match args.as_slice() {
                [] => return Ok(Value::Number(this.fraction())),
                [n] => (1, bound(n).map_err(fail)?),
                [m, n] => (bound(m).map_err(fail)?, bound(n).map_err(fail)?),
                _ => {
                    return Err(fail(format!(
                        "takes at most two numbers, and {} were given",
                        args.len()
                    )));
                }
            };
            if lo > hi {
                return Err(fail(format!(
                    "from {lo} to {hi} has no whole number to draw"
                )));
            }
            Ok(Value::Number(this.whole(lo, hi) as f64))
        });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!("random({})", this.shown))
        });
    }
}

/// `random(seed)`: a new generator; a missing or unusable seed raises.
pub(crate) fn random(lua: &Lua, seed: Value) -> mlua::Result<mlua::AnyUserData> {
    let Some((bits, shown)) = seed_of(&seed) else {
        let why = match seed {
            Value::Nil => {
                "`random` needs a seed — random(1) draws the same numbers every run".into()
            }
            _ => "`random`'s seed is a number or a string".to_owned(),
        };
        return Err(crate::run::raise(lua, why));
    };
    lua.create_userdata(Rng::new(bits, shown))
}
