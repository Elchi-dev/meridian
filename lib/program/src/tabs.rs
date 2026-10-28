//! Tab stops, as expand and unexpand take them: every so many columns,
//! eight unless told otherwise, or at the columns a list names and
//! nowhere past its last.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Where tabs stop, counting columns from 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tabs {
    /// Every so many columns.
    Every(u64),
    /// At these columns, going up.
    At(Vec<u64>),
}

impl Default for Tabs {
    fn default() -> Self {
        Tabs::Every(8)
    }
}

impl Tabs {
    /// The tab stops `-t` names to the tool `tool`: one number for every
    /// so many columns, or a list of them going up, apart by commas or
    /// blanks.
    ///
    /// # Errors
    ///
    /// What the tool says about stops it cannot take.
    pub fn parse(tool: &str, text: &str) -> Result<Tabs, String> {
        let mut stops: Vec<u64> = Vec::new();
        let words = text.split([',', ' ', '\t']).filter(|word| !word.is_empty());
        for word in words {
            let stop = Some(word)
                .filter(|w| w.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|w| w.parse::<u64>().ok())
                .filter(|&stop| stop > 0)
                .ok_or_else(|| format!("{tool}: {word} is no tab stop\n"))?;
            if stops.last().is_some_and(|&last| stop <= last) {
                return Err(format!("{tool}: tab stops go up, and {stop} does not\n"));
            }
            stops.push(stop);
        }
        match stops.as_slice() {
            [] => Err(format!("{tool}: -t wants tab stops\n")),
            [every] => Ok(Tabs::Every(*every)),
            _ => Ok(Tabs::At(stops)),
        }
    }

    /// The first stop past `column`, none past the last of a list.
    #[must_use]
    pub fn after(&self, column: u64) -> Option<u64> {
        match self {
            Tabs::Every(every) => (column / every).checked_add(1)?.checked_mul(*every),
            Tabs::At(stops) => stops.iter().copied().find(|&stop| stop > column),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_columns_apart_unless_told_otherwise() {
        let tabs = Tabs::default();
        assert_eq!(tabs.after(0), Some(8));
        assert_eq!(tabs.after(7), Some(8));
        assert_eq!(tabs.after(8), Some(16));
        assert_eq!(Tabs::Every(3).after(4), Some(6));
        assert_eq!(Tabs::Every(1).after(9), Some(10));
        assert_eq!(Tabs::Every(u64::MAX).after(1), Some(u64::MAX));
        assert_eq!(
            Tabs::Every(u64::MAX).after(u64::MAX),
            None,
            "past the last column"
        );
        assert_eq!(Tabs::Every(4).after(u64::MAX - 1), None);
    }

    #[test]
    fn a_list_stops_at_its_columns_and_nowhere_past_them() {
        let tabs = Tabs::At(alloc::vec![2, 5, 9]);
        assert_eq!(tabs.after(0), Some(2));
        assert_eq!(tabs.after(1), Some(2));
        assert_eq!(tabs.after(2), Some(5));
        assert_eq!(tabs.after(8), Some(9));
        assert_eq!(tabs.after(9), None);
        assert_eq!(tabs.after(100), None);
    }

    #[test]
    fn stops_read_as_one_number_or_a_list() {
        assert_eq!(Tabs::parse("x", "4"), Ok(Tabs::Every(4)));
        assert_eq!(Tabs::parse("x", " 12 "), Ok(Tabs::Every(12)));
        assert_eq!(Tabs::parse("x", "2,5"), Ok(Tabs::At(alloc::vec![2, 5])));
        assert_eq!(
            Tabs::parse("x", "2 5\t9"),
            Ok(Tabs::At(alloc::vec![2, 5, 9]))
        );
        assert_eq!(Tabs::parse("x", "1,,3, "), Ok(Tabs::At(alloc::vec![1, 3])));
    }

    #[test]
    fn stops_that_do_not_read() {
        let wrong = |text| Tabs::parse("expand", text).unwrap_err();
        assert_eq!(wrong("0"), "expand: 0 is no tab stop\n");
        assert_eq!(wrong("2,0"), "expand: 0 is no tab stop\n");
        assert_eq!(wrong("+3"), "expand: +3 is no tab stop\n");
        assert_eq!(wrong("-3"), "expand: -3 is no tab stop\n");
        assert_eq!(wrong("3a"), "expand: 3a is no tab stop\n");
        assert_eq!(
            wrong("99999999999999999999"),
            "expand: 99999999999999999999 is no tab stop\n"
        );
        assert_eq!(wrong("3,2"), "expand: tab stops go up, and 2 does not\n");
        assert_eq!(wrong("3,3"), "expand: tab stops go up, and 3 does not\n");
        assert_eq!(wrong(""), "expand: -t wants tab stops\n");
        assert_eq!(
            Tabs::parse("unexpand", ", ,").unwrap_err(),
            "unexpand: -t wants tab stops\n"
        );
    }
}
