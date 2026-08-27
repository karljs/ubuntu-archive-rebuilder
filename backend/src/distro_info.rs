//! Ubuntu release metadata from distro-info's ubuntu.csv.

use chrono::NaiveDate;

pub const DISTRO_INFO_CSV: &str = "/usr/share/distro-info/ubuntu.csv";

pub struct SeriesRow {
    pub version: String,
    pub series: String,
    pub release: Option<String>,
    pub eol: Option<String>,
}

// CSV rows carry a varying number of trailing columns; index by header name
// and tolerate missing fields.
pub fn parse_csv(csv: &str) -> Vec<SeriesRow> {
    let mut lines = csv.lines();
    let Some(header) = lines.next() else {
        return vec![];
    };
    let cols: Vec<&str> = header.split(',').map(str::trim).collect();
    let idx = |name: &str| cols.iter().position(|c| *c == name);
    let (iv, is, ir, ie) = (idx("version"), idx("series"), idx("release"), idx("eol"));

    lines
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            let get = |i: Option<usize>| {
                i.and_then(|i| f.get(i))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            };
            Some(SeriesRow {
                version: get(iv).unwrap_or_default(),
                series: get(is)?,
                release: get(ir),
                eol: get(ie),
            })
        })
        .collect()
}

/// Released LTS releases in standard support; file order = release order.
pub fn maintained_lts(rows: &[SeriesRow], today: NaiveDate) -> Vec<String> {
    rows.iter()
        .filter(|r| r.version.contains("LTS"))
        .filter(|r| date(&r.release).is_some_and(|d| d <= today))
        .filter(|r| date(&r.eol).is_some_and(|d| d > today))
        .map(|r| r.series.clone())
        .collect()
}

fn date(s: &Option<String>) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.as_deref()?, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_csv_preserves_file_order() {
        let csv = "version,codename,series,created,release,eol\n\
                   24.04 LTS,Noble Numbat,noble,2023-10-12,2024-04-25,2029-05-31\n\
                   24.10,Oracular Oriole,oracular,2024-04-25,2024-10-10,2025-07-10\n";
        let rows = parse_csv(csv);
        let series: Vec<&str> = rows.iter().map(|r| r.series.as_str()).collect();
        assert_eq!(series, vec!["noble", "oracular"]);
        assert_eq!(rows[0].version, "24.04 LTS");
        assert_eq!(rows[0].release.as_deref(), Some("2024-04-25"));
        assert_eq!(rows[0].eol.as_deref(), Some("2029-05-31"));
    }

    #[test]
    fn parse_csv_tolerates_missing_columns() {
        let csv = "version,codename,series\n26.04 LTS,Resolute Raccoon,resolute\n";
        let rows = parse_csv(csv);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].series, "resolute");
        assert_eq!(rows[0].release, None);
        assert_eq!(rows[0].eol, None);
    }

    #[test]
    fn maintained_lts_filters_correctly() {
        let csv = "version,codename,series,created,release,eol\n\
                   20.04 LTS,Focal Fossa,focal,2019-10-17,2020-04-23,2025-04-23\n\
                   22.04 LTS,Jammy Jellyfish,jammy,2021-10-14,2022-04-21,2027-06-01\n\
                   22.10,Kinetic Kudu,kinetic,2022-04-28,2022-10-20,2023-07-20\n\
                   24.04 LTS,Noble Numbat,noble,2023-10-12,2024-04-25,2029-05-31\n\
                   27.04 LTS,Future Fox,fox,2026-10-15,2027-04-22,2032-04-01\n";
        let rows = parse_csv(csv);
        let today = NaiveDate::from_ymd_opt(2026, 8, 27).unwrap();
        let got = maintained_lts(&rows, today);
        assert_eq!(got, vec!["jammy".to_string(), "noble".to_string()]);
    }
}
