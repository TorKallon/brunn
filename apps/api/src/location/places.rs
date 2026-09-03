#[derive(Clone, Debug, PartialEq)]
pub struct KnownPlace {
    pub label: String,
    pub kind: String,
    pub lat: f64,
    pub lon: f64,
    pub radius_m: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlacesWarning {
    Missing,
    Unparseable,
    InvalidRows,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedPlaces {
    pub places: Vec<KnownPlace>,
    /// At most one content-free warning for the caller to emit for this parse.
    pub warning: Option<PlacesWarning>,
}

pub fn parse_places(text: Option<&str>) -> ParsedPlaces {
    let Some(text) = text else {
        return ParsedPlaces {
            warning: Some(PlacesWarning::Missing),
            ..ParsedPlaces::default()
        };
    };
    let lines = text.lines().collect::<Vec<_>>();
    let Some((header_index, columns)) = lines.iter().enumerate().find_map(|(index, line)| {
        let cells = table_cells(line)?;
        let separator = lines.get(index + 1).and_then(|line| table_cells(line))?;
        if cells.len() != separator.len() || !separator.iter().all(|cell| is_separator_cell(cell)) {
            return None;
        }
        required_columns(&cells).map(|columns| (index, columns))
    }) else {
        return ParsedPlaces {
            warning: Some(PlacesWarning::Unparseable),
            ..ParsedPlaces::default()
        };
    };

    let mut places = Vec::new();
    let mut invalid_rows = false;
    for line in lines.iter().skip(header_index + 2) {
        let Some(cells) = table_cells(line) else {
            break;
        };
        let parsed = parse_place_row(&cells, columns);
        match parsed {
            Some(place) => places.push(place),
            None => invalid_rows = true,
        }
    }
    ParsedPlaces {
        places,
        warning: invalid_rows.then_some(PlacesWarning::InvalidRows),
    }
}

#[derive(Clone, Copy)]
struct RequiredColumns {
    label: usize,
    kind: usize,
    lat: usize,
    lon: usize,
    radius_m: usize,
}

fn required_columns(cells: &[&str]) -> Option<RequiredColumns> {
    let find = |name: &str| {
        cells
            .iter()
            .position(|cell| cell.trim().eq_ignore_ascii_case(name))
    };
    Some(RequiredColumns {
        label: find("Label")?,
        kind: find("Kind")?,
        lat: find("Lat")?,
        lon: find("Lon")?,
        radius_m: find("Radius m")?,
    })
}

fn parse_place_row(cells: &[&str], columns: RequiredColumns) -> Option<KnownPlace> {
    let label = cells.get(columns.label)?.trim();
    if label.is_empty() {
        return None;
    }
    let lat = cells.get(columns.lat)?.trim().parse::<f64>().ok()?;
    let lon = cells.get(columns.lon)?.trim().parse::<f64>().ok()?;
    let radius_m = cells.get(columns.radius_m)?.trim().parse::<u16>().ok()?;
    if !lat.is_finite()
        || !lon.is_finite()
        || !(-90.0..=90.0).contains(&lat)
        || !(-180.0..=180.0).contains(&lon)
        || !(50..=15_000).contains(&radius_m)
    {
        return None;
    }
    let kind = cells.get(columns.kind)?.trim();
    Some(KnownPlace {
        label: label.to_owned(),
        kind: if kind.is_empty() {
            "other".to_owned()
        } else {
            kind.to_ascii_lowercase()
        },
        lat,
        lon,
        radius_m,
    })
}

fn table_cells(line: &str) -> Option<Vec<&str>> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('|').unwrap_or(trimmed);
    let cells = trimmed.split('|').map(str::trim).collect::<Vec<_>>();
    (cells.len() >= 2).then_some(cells)
}

fn is_separator_cell(cell: &str) -> bool {
    let cell = cell.trim().trim_start_matches(':').trim_end_matches(':');
    cell.len() >= 3 && cell.bytes().all(|byte| byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_first_matching_table_with_case_insensitive_reordered_headers() {
        let parsed = parse_places(Some(
            "# Places\n\n| Noise | Value |\n| --- | --- |\n| Label | ignored |\n\n\
             | LON | Radius M | label | LAT | KIND | Note |\n\
             | --- | ---: | :--- | --- | --- | --- |\n\
             | -122.2035 | 150 | Home | 47.6156 | HOME | canonical |\n\
             | -122.4000 | 200 | Office | 37.7000 |  | optional kind |\n\n\
             | Label | Kind | Lat | Lon | Radius m |\n\
             | --- | --- | --- | --- | --- |\n\
             | Later | other | 1 | 2 | 100 |",
        ));

        assert_eq!(parsed.warning, None);
        assert_eq!(parsed.places.len(), 2);
        assert_eq!(parsed.places[0].label, "Home");
        assert_eq!(parsed.places[0].kind, "home");
        assert_eq!(parsed.places[1].kind, "other");
    }

    #[test]
    fn skips_invalid_rows_and_returns_one_content_free_warning() {
        let parsed = parse_places(Some(
            "| Label | Kind | Lat | Lon | Radius m |\n\
             | --- | --- | --- | --- | --- |\n\
             | Home | home | 47.6 | -122.2 | 150 |\n\
             |  | work | 37.7 | -122.4 | 200 |\n\
             | Bad latitude | other | 91 | 0 | 100 |\n\
             | Fractional radius | other | 0 | 0 | 50.5 |\n\
             | Too small | other | 0 | 0 | 49 |\n\
             | Too wide | other | 0 | 0 | 15001 |",
        ));

        assert_eq!(parsed.places.len(), 1);
        assert_eq!(parsed.warning, Some(PlacesWarning::InvalidRows));
    }

    #[test]
    fn radius_cap_is_fifteen_kilometres() {
        let parsed = parse_places(Some(
            "| Label | Kind | Lat | Lon | Radius m |\n\
             | --- | --- | --- | --- | --- |\n\
             | Whistler | resort | 50.1 | -122.9 | 15000 |\n\
             | Crystal Mountain | resort | 46.9 | -121.4 | 4000 |",
        ));

        assert_eq!(parsed.warning, None);
        assert_eq!(
            parsed
                .places
                .iter()
                .map(|place| place.radius_m)
                .collect::<Vec<_>>(),
            vec![15_000, 4_000]
        );
    }

    #[test]
    fn missing_and_unparseable_inputs_return_no_places_and_one_warning() {
        let missing = parse_places(None);
        assert!(missing.places.is_empty());
        assert_eq!(missing.warning, Some(PlacesWarning::Missing));

        let unparseable = parse_places(Some("# Places\n\nNo table here."));
        assert!(unparseable.places.is_empty());
        assert_eq!(unparseable.warning, Some(PlacesWarning::Unparseable));
    }

    #[test]
    fn header_only_places_file_is_valid_and_empty() {
        let parsed = parse_places(Some(
            "---\nkind: location-places\n---\n\
             | Label | Kind | Lat | Lon | Radius m |\n\
             | --- | --- | --- | --- | --- |",
        ));
        assert!(parsed.places.is_empty());
        assert_eq!(parsed.warning, None);
    }
}
