use chrono::{DateTime, Datelike, Days, NaiveDate, TimeZone};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Today,
    Week,
    Month,
    Year,
    All,
    Custom,
}
#[derive(Clone, Debug)]
pub struct DateWindow {
    start: Option<i64>,
    end: Option<i64>,
    zone: String,
    previous: Option<Box<DateWindow>>,
}
impl DateWindow {
    pub fn all(zone: impl Into<String>) -> Self {
        Self {
            start: None,
            end: None,
            zone: zone.into(),
            previous: None,
        }
    }
    pub fn start(&self) -> Option<i64> {
        self.start
    }
    pub fn end(&self) -> Option<i64> {
        self.end
    }
    pub fn timezone(&self) -> &str {
        &self.zone
    }
    pub fn previous(&self) -> Option<&Self> {
        self.previous.as_deref()
    }
    pub fn contains(&self, timestamp: i64) -> bool {
        self.start.is_none_or(|s| timestamp >= s) && self.end.is_none_or(|e| timestamp < e)
    }
    /// Inclusive calendar dates; converts each boundary independently through the desktop timezone.
    pub fn calendar<T: TimeZone>(
        period: Period,
        now: DateTime<T>,
        custom: Option<(NaiveDate, NaiveDate)>,
    ) -> Result<Self, String>
    where
        T::Offset: std::fmt::Display,
    {
        let zone = now.timezone();
        let name = now.format("%Z").to_string();
        if period == Period::All {
            return Ok(Self::all(name));
        }
        let today = now.date_naive();
        let tomorrow = today
            .checked_add_days(Days::new(1))
            .ok_or("date overflow")?;
        let (start, end) = match period {
            Period::Today => (today, tomorrow),
            Period::Week => (
                today
                    .checked_sub_days(Days::new(u64::from(today.weekday().num_days_from_monday())))
                    .ok_or("date overflow")?,
                tomorrow,
            ),
            Period::Month => (today.with_day(1).ok_or("invalid month")?, tomorrow),
            Period::Year => (
                NaiveDate::from_ymd_opt(today.year(), 1, 1).ok_or("invalid year")?,
                tomorrow,
            ),
            Period::Custom => {
                let (start, end) = custom.ok_or("select both calendar dates")?;
                if start > end {
                    return Err("start date must not follow end date".into());
                }
                (
                    start,
                    end.checked_add_days(Days::new(1)).ok_or("date overflow")?,
                )
            }
            Period::All => unreachable!(),
        };
        let midnight = |date: NaiveDate| {
            zone.from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
                .earliest()
                .map(|v| v.timestamp())
                .ok_or_else(|| "calendar boundary is unavailable in desktop timezone".to_string())
        };
        let days = (end - start).num_days() as u64;
        let (prior_start, prior_end) = match period {
            Period::Week => {
                let prior = start
                    .checked_sub_days(Days::new(7))
                    .ok_or("date overflow")?;
                (
                    prior,
                    prior
                        .checked_add_days(Days::new(days))
                        .ok_or("date overflow")?,
                )
            }
            Period::Month => {
                let end = start;
                let prior = end
                    .checked_sub_days(Days::new(1))
                    .ok_or("date overflow")?
                    .with_day(1)
                    .ok_or("invalid month")?;
                let end = prior
                    .checked_add_days(Days::new(days))
                    .ok_or("date overflow")?
                    .min(end);
                (prior, end)
            }
            Period::Year => {
                let prior =
                    NaiveDate::from_ymd_opt(start.year() - 1, 1, 1).ok_or("invalid year")?;
                (
                    prior,
                    prior
                        .checked_add_days(Days::new(days))
                        .ok_or("date overflow")?
                        .min(start),
                )
            }
            _ => (
                start
                    .checked_sub_days(Days::new(days))
                    .ok_or("date overflow")?,
                start,
            ),
        };
        Ok(Self {
            start: Some(midnight(start)?),
            end: Some(midnight(end)?),
            zone: name.clone(),
            previous: Some(Box::new(Self {
                start: Some(midnight(prior_start)?),
                end: Some(midnight(prior_end)?),
                zone: name,
                previous: None,
            })),
        })
    }
}
