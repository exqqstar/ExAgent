use time::OffsetDateTime;

pub fn now_unix_millis() -> i64 {
    let nanos = OffsetDateTime::now_utc().unix_timestamp_nanos();
    (nanos / 1_000_000) as i64
}
