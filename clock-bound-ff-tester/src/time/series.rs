//! Abstraction around time series data points

use serde::{Deserialize, Serialize};

use super::inner::{Diff, Time, Type};

/// One-dimensional array of data (great for time series)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SeriesDeserialize<X, Y>")]
pub struct Series<X, Y> {
    indices: Vec<X>,
    data: Vec<Y>,
}

impl<X, Y> Series<X, Y> {
    /// Create a new series
    ///
    /// Returns none if index and data lengths don't match
    pub fn new(index: Vec<X>, data: Vec<Y>) -> Option<Self> {
        if index.len() != data.len() {
            return None;
        }
        Some(Self {
            indices: index,
            data,
        })
    }

    /// Returns the number of data points in this series
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Returns `true` if this series has no data points
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Get the indices of the series
    pub fn indices(&self) -> &[X] {
        &self.indices
    }

    /// Get the data of the series
    pub fn data(&self) -> &[Y] {
        &self.data
    }

    /// Iterator of index with data
    pub fn iter(&self) -> impl Iterator<Item = (&X, &Y)> {
        self.indices.iter().zip(self.data.iter())
    }

    /// Construct from an iterator of tuples
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = (X, Y)>,
    {
        let iter = iter.into_iter();
        let capacity = iter.size_hint().0;
        let mut index = Vec::with_capacity(capacity);
        let mut data = Vec::with_capacity(capacity);
        for (i, d) in iter {
            index.push(i);
            data.push(d);
        }
        Self {
            indices: index,
            data,
        }
    }
}

impl<X, Y> FromIterator<(X, Y)> for Series<X, Y> {
    fn from_iter<T: IntoIterator<Item = (X, Y)>>(iter: T) -> Self {
        Self::from_iter(iter)
    }
}

impl<T: Type + Copy, Y> Series<Diff<T>, Y> {
    /// Get the absolute time of a time series of durations
    ///
    /// Useful for ad-hoc conversion between a generic time series of durations and
    /// a series with a known starting time
    pub fn absolute_time_indexes(&self, start_time: Time<T>) -> impl Iterator<Item = Time<T>> {
        self.indices.iter().map(move |offset| start_time + *offset)
    }

    /// [`Series::iter`] but with [`Time`] values on the X axis
    pub fn instant_iter(&self, start_time: Time<T>) -> impl Iterator<Item = (Time<T>, &Y)> {
        self.absolute_time_indexes(start_time).zip(self.data.iter())
    }
}

// helper to guard against invariants when deserializing
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SeriesDeserialize<X, Y> {
    pub indices: Vec<X>,
    pub data: Vec<Y>,
}

impl<X, Y> TryFrom<SeriesDeserialize<X, Y>> for Series<X, Y> {
    type Error = Error;

    fn try_from(value: SeriesDeserialize<X, Y>) -> Result<Self, Self::Error> {
        if value.indices.len() != value.data.len() {
            return Err(Error {
                index_len: value.indices.len(),
                data_len: value.data.len(),
            });
        }
        Ok(Self {
            indices: value.indices,
            data: value.data,
        })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("offsets and time_steps length mismatch: index len: {index_len}. data len: {data_len}")]
struct Error {
    index_len: usize,
    data_len: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_creation_success() {
        let index = vec![1, 2, 3];
        let data = vec!["a", "b", "c"];

        let series = Series::new(index.clone(), data.clone());
        assert!(series.is_some());

        let series = series.unwrap();
        assert_eq!(series.indices(), &index);
        assert_eq!(series.data(), &data);
    }

    #[test]
    fn series_creation_failure() {
        let index = vec![1, 2, 3];
        let data = vec!["a", "b"]; // Mismatched lengths

        let series = Series::new(index, data);
        assert!(series.is_none());
    }

    #[test]
    fn series_deserialization_success() {
        let deser = SeriesDeserialize {
            indices: vec![1, 2],
            data: vec!["a", "b"],
        };

        let _ = Series::try_from(deser).unwrap();
    }

    #[test]
    fn series_deserialization_failure() {
        let deser = SeriesDeserialize {
            indices: vec![1, 2, 3],
            data: vec!["a", "b"],
        };

        let result: Result<Series<_, _>, _> = Series::try_from(deser);
        assert!(result.is_err());

        if let Err(err) = result {
            assert_eq!(err.index_len, 3);
            assert_eq!(err.data_len, 2);
        }
    }

    #[test]
    fn serialization_loopback() {
        let series = Series::new(vec![1, 2, 3], vec!["a", "b", "c"]).unwrap();
        let serialized = serde_json::to_string(&series).unwrap();
        let deserialized: SeriesDeserialize<_, _> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(series, Series::try_from(deserialized).unwrap());
    }
}
