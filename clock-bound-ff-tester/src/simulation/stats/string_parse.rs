use super::GammaDistribution;
use std::str::FromStr;

impl FromStr for GammaDistribution {
    type Err = String;

    /// Function parses a string and if successful returns a `GammaDistribution` struct.
    ///
    /// Currently this function assumes that the args in the string is ordered.
    /// The format is: {`shape`,`rate`,`loc`}
    ///
    /// Example:
    /// {1.0,2.0,1}
    ///
    /// Where:
    ///     `shape`: 1.0
    ///     `rate`: 2.0
    ///     `loc`: 1.0
    ///
    /// # Errors
    /// If input fails to parse the specified format
    fn from_str(s: &str) -> Result<Self, String> {
        let [shape, rate, loc] = {
            let rv = s
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .ok_or(String::from(
                    "Parsing distribution. Expected braces. No braces found.",
                ))?;
            let rv: Vec<&str> = rv.split(',').collect();

            if rv.len() > 3 {
                return Err(format!(
                    "Parsing distribution. Too many inputs. Expected 3. Found {}",
                    rv.len()
                ));
            }
            if rv.len() < 3 {
                return Err(format!(
                    "Parsing distribution. Too few inputs. Expected 3. Found {}",
                    rv.len()
                ));
            }

            [rv[0], rv[1], rv[2]]
        };

        let shape = shape
            .trim()
            .parse::<f64>()
            .map_err(|_| String::from("ParseDelayArgs::ParseParam"))?;
        let rate = rate
            .trim()
            .parse::<f64>()
            .map_err(|_| String::from("ParseDelayArgs::ParseParam"))?;
        let loc = loc
            .trim()
            .parse::<f64>()
            .map_err(|_| String::from("ParseDelayArgs::ParseParam"))?;

        Result::Ok(GammaDistribution::new(shape, rate, loc).unwrap())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::floats("{1.0,2.0,3.0}", (1.0, 2.0, 3.0))]
    #[case::ints("{1,2,3}", (1.0, 2.0, 3.0))]
    #[case::whitespace("{1, 2, 3 }", (1.0, 2.0, 3.0))]
    fn parsing_success(#[case] input: &str, #[case] expected: (f64, f64, f64)) {
        let d = GammaDistribution::from_str(input).unwrap();

        assert!(approx::relative_eq!(d.shape(), expected.0));
        assert!(approx::relative_eq!(d.rate(), expected.1));
        assert!(approx::relative_eq!(d.loc(), expected.2));
    }

    #[rstest]
    #[case::missing_first_paran("1.0,2.0,3.0}")]
    #[case::second_first_paran("{1.0,2.0,3.0")]
    #[case::too_few("{1.0,2.0}")]
    #[case::too_many("{1.0,1.0,1.0,2.0}")]
    #[case::missing_comma("{1.0,1.02.0,3.0}")]
    fn parsing_failure(#[case] input: &str) {
        GammaDistribution::from_str(input).unwrap_err();
    }
}
