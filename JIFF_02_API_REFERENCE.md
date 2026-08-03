# jiff 0.2.35 API Reference (Local Source)

## SignedDuration Constructors (static)
| Method | Signature | Notes |
|--------|-----------|-------|
| `new` | `pub const fn new(mut secs: i64, mut nanos: i32) -> SignedDuration` | |
| `from_secs` | `pub const fn from_secs(secs: i64) -> SignedDuration` | |
| `from_millis` | `pub const fn from_millis(millis: i64) -> SignedDuration` | |
| `from_millis_i128` | `pub const fn from_millis_i128(millis: i128) -> SignedDuration` | |
| `from_micros` | `pub const fn from_micros(micros: i64) -> SignedDuration` | |
| `from_micros_i128` | `pub const fn from_micros_i128(micros: i128) -> SignedDuration` | |
| `from_nanos` | `pub const fn from_nanos(nanos: i64) -> SignedDuration` | |
| `from_nanos_i128` | `pub const fn from_nanos_i128(nanos: i128) -> SignedDuration` | |
| `from_hours` | `pub const fn from_hours(hours: i64) -> SignedDuration` | |
| `from_mins` | `pub const fn from_mins(mins: i64) -> SignedDuration` | |
| `from_secs_f64` | `pub fn from_secs_f64(secs: f64) -> SignedDuration` | |
| `from_secs_f32` | `pub fn from_secs_f32(secs: f32) -> SignedDuration` | |
| `try_from_hours` | `pub const fn try_from_hours(hours: i64) -> Option<SignedDuration>` | |
| `try_from_mins` | `pub const fn try_from_mins(mins: i64) -> Option<SignedDuration>` | |
| `try_from_millis_i128` | `pub const fn try_from_millis_i128(millis: i128) -> Option<SignedDuration>` | |
| `try_from_micros_i128` | `pub const fn try_from_micros_i128(micros: i128) -> Option<SignedDuration>` | |
| `try_from_nanos_i128` | `pub const fn try_from_nanos_i128(nanos: i128) -> Option<SignedDuration>` | |

**REMEMBER**: `from_millis` NOT `from_millisecond`, `from_secs` NOT `from_seconds`, `from_mins` NOT `from_minutes`

## SignedDuration Query Methods
| Method | Signature |
|--------|-----------|
| `is_zero` | `pub const fn is_zero(&self) -> bool` |
| `as_secs` | `pub const fn as_secs(&self) -> i64` |
| `subsec_millis` | `pub const fn subsec_millis(&self) -> i32` |
| `subsec_micros` | `pub const fn subsec_micros(&self) -> i32` |
| `subsec_nanos` | `pub const fn subsec_nanos(&self) -> i32` |
| `as_millis` | `pub const fn as_millis(&self) -> i128` |
| `as_micros` | `pub const fn as_micros(&self) -> i128` |
| `as_nanos` | `pub const fn as_nanos(&self) -> i128` |
| `as_secs_f64` | `pub fn as_secs_f64(&self) -> f64` |
| `as_secs_f32` | `pub fn as_secs_f32(&self) -> f32` |
| `as_millis_f64` | `pub fn as_millis_f64(&self) -> f64` |
| `as_millis_f32` | `pub fn as_millis_f32(&self) -> f32` |
| `as_hours` | `pub const fn as_hours(&self) -> i64` |
| `as_mins` | `pub const fn as_mins(&self) -> i64` |
| `signum` | `pub const fn signum(self) -> i8` |
| `is_positive` | `pub const fn is_positive(&self) -> bool` |
| `is_negative` | `pub const fn is_negative(&self) -> bool` |

## SignedDuration Arithmetic
| Method | Signature |
|--------|-----------|
| `checked_add` | `pub const fn checked_add(&self, rhs: SignedDuration) -> Option<SignedDuration>` |
| `saturating_add` | `pub const fn saturating_add(self, rhs: SignedDuration) -> SignedDuration` |
| `checked_sub` | `pub const fn checked_sub(&self, rhs: SignedDuration) -> Option<SignedDuration>` |
| `saturating_sub` | `pub const fn saturating_sub(self, rhs: SignedDuration) -> SignedDuration` |
| `checked_mul` | `pub const fn checked_mul(self, rhs: i32) -> Option<SignedDuration>` |
| `saturating_mul` | `pub const fn saturating_mul(self, rhs: i32) -> SignedDuration` |
| `checked_div` | `pub const fn checked_div(self, rhs: i32) -> Option<SignedDuration>` |
| `checked_neg` | `pub const fn checked_neg(self) -> Option<SignedDuration>` |
| `abs` | `pub const fn abs(self) -> SignedDuration` |
| `mul_f64` | `pub fn mul_f64(self, rhs: f64) -> SignedDuration` |
| `mul_f32` | `pub fn mul_f32(self, rhs: f32) -> SignedDuration` |
| `div_f64` | `pub fn div_f64(self, rhs: f64) -> SignedDuration` |
| `div_f32` | `pub fn div_f32(self, rhs: f32) -> SignedDuration` |
| `div_duration_f64` | `pub fn div_duration_f64(self, rhs: SignedDuration) -> f64` |
| `div_duration_f32` | `pub fn div_duration_f32(self, rhs: SignedDuration) -> f32` |

## Timestamp Constructors
| Method | Signature |
|--------|-----------|
| `now` | `pub fn now() -> Timestamp` |
| `new` | `pub fn new(second: i64, nanosecond: i32) -> Result<Timestamp, Error>` |
| `constant` | `pub const fn constant(second: i64, nanosecond: i32) -> Timestamp` |
| `from_second` | `pub fn from_second(second: i64) -> Result<Timestamp, Error>` |
| `from_millisecond` | `pub fn from_millisecond(millisecond: i64) -> Result<Timestamp, Error>` |
| `from_microsecond` | `pub fn from_microsecond(microsecond: i64) -> Result<Timestamp, Error>` |
| `from_nanosecond` | `pub fn from_nanosecond(nanosecond: i128) -> Result<Timestamp, Error>` |
| `from_duration` | `pub fn from_duration(timestamp: i64, duration: SignedDuration) -> Result<Timestamp, Error>` |

## Timestamp Query
| Method | Signature |
|--------|-----------|
| `as_second` | `pub fn as_second(self) -> i64` |
| `as_millisecond` | `pub fn as_millisecond(self) -> i64` |
| `as_microsecond` | `pub fn as_microsecond(self) -> i64` |
| `as_nanosecond` | `pub fn as_nanosecond(self) -> i128` |
| `subsec_millisecond` | `pub fn subsec_millisecond(self) -> i32` |
| `subsec_microsecond` | `pub fn subsec_microsecond(self) -> i32` |
| `subsec_nanosecond` | `pub fn subsec_nanosecond(self) -> i32` |
| `as_duration` | `pub fn as_duration(self) -> SignedDuration` |
| `signum` | `pub fn signum(self) -> i8` |
| `is_zero` | `pub fn is_zero(self) -> bool` |

## Timestamp Arithmetic
| Method | Signature |
|--------|-----------|
| `checked_add<A>` | `pub fn checked_add<A: Into<TimestampArithmetic>>(&self, arithmetic: A) -> Option<Timestamp>` |
| `checked_sub<A>` | `pub fn checked_sub<A: Into<TimestampArithmetic>>(&self, arithmetic: A) -> Option<Timestamp>` |
| `saturating_add<A>` | `pub fn saturating_add<A: Into<TimestampArithmetic>>(&self, arithmetic: A) -> Timestamp` |
| `saturating_sub<A>` | `pub fn saturating_sub<A: Into<TimestampArithmetic>>(&self, arithmetic: A) -> Timestamp` |
| `until<A>` | `pub fn until<A: Into<TimestampDifference>>(&self, dt: A) -> Result<TimestampDifference, Error>` |
| `since<A>` | `pub fn since<A: Into<TimestampDifference>>(&self, dt: A) -> Result<TimestampDifference, Error>` |
| `duration_until` | `pub fn duration_until(self, other: Timestamp) -> SignedDuration` |
| `duration_since` | `pub fn duration_since(self, other: Timestamp) -> SignedDuration` |

## Timestamp Conversion
| Method | Signature |
|--------|-----------|
| `in_tz` | `pub fn in_tz(self, time_zone_name: &str) -> Result<Zoned, Error>` |
| `to_zoned` | `pub fn to_zoned(self, tz: TimeZone) -> Zoned` |

## Timestamp Formatting
| Method | Signature |
|--------|-----------|
| `strftime<'f>` | `pub fn strftime<'f, F: 'f + ?Sized + AsRef<[u8]>>(&self, fmt: F) -> Result<String, Error>` |
| `display_with_offset` | `pub fn display_with_offset<'a>(&'a self) -> impl Display + 'a` |

## Common Migration: jiff 0.1 → 0.2
| Was (0.1) | Now (0.2) |
|-----------|-----------|
| `SignedDuration::from_millisecond(n)` | `SignedDuration::from_millis(n)` |
| `SignedDuration::from_seconds(n)` | `SignedDuration::from_secs(n)` |
| `SignedDuration::from_minutes(n)` | `SignedDuration::from_mins(n)` |
| `SignedDuration::from_hour(n)` | `SignedDuration::from_hours(n)` |
| `SignedDuration::from_microsecond(n)` | `SignedDuration::from_micros(n)` |
| `SignedDuration::from_nanosecond(n)` | `SignedDuration::from_nanos(n)` |
| `Timestamp::epoch()` | `Timestamp::now()` or `Timestamp::new(0, 0)?` |

