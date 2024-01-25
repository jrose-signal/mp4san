#![allow(missing_docs)]

use std::num::NonZeroU32;

use mediasan_common::{Report, ResultExt as _};

use super::error::ParseResultExt as _;
use super::{BoundedArray, BoxType, ConstFullBoxHeader, ConstU32, Mp4Value, ParseBox, ParseError, ParsedBox};
use crate::error::Result;

#[derive(Clone, Debug, ParsedBox)]
#[box_type = "stsz"]
pub enum StszBox {
    FixedSize {
        _parsed_header: ConstFullBoxHeader,
        size: NonZeroU32,
        number_of_samples: u32,
    },
    VariableSize {
        _parsed_header: ConstFullBoxHeader,
        _not_fixed_size: ConstU32,
        entries: BoundedArray<u32, u32>,
    },
}

impl ParseBox for StszBox {
    const NAME: BoxType = BoxType::FourCC(mp4san::parse::FourCC { value: *b"stsz" });
    fn parse(buf: &mut bytes::BytesMut) -> Result<Self, ParseError> {
        let parsed_header =
            ConstFullBoxHeader::parse(&mut *buf).while_parsing_field(Self::NAME, stringify!(_parsed_header))?;
        let size = u32::parse(&mut *buf)
            .while_parsing_field(Self::NAME, stringify!(size))?
            .try_into();

        let result = match size {
            Ok(size) => {
                let number_of_samples =
                    Mp4Value::parse(&mut *buf).while_parsing_field(Self::NAME, stringify!(number_of_samples))?;
                Self::FixedSize { _parsed_header: parsed_header, size, number_of_samples }
            }
            Err(_) => {
                let entries = Mp4Value::parse(&mut *buf).while_parsing_field(Self::NAME, stringify!(entries))?;

                Self::VariableSize { _parsed_header: parsed_header, _not_fixed_size: Default::default(), entries }
            }
        };

        if !buf.is_empty() {
            return Err(Report::from(ParseError::InvalidInput))
                .attach_printable(format!("{} bytes of extra unparsed data", buf.len()))
                .while_parsing_box(<StszBox>::NAME);
        }
        Ok(result)
    }
}

impl Default for StszBox {
    fn default() -> Self {
        Self::VariableSize {
            _parsed_header: Default::default(),
            _not_fixed_size: Default::default(),
            entries: Default::default(),
        }
    }
}

impl StszBox {
    pub fn sample_sizes(&self) -> impl ExactSizeIterator<Item = Result<u32, ParseError>> + '_ {
        let (mut variable_iter, fixed_size, count) = match self {
            StszBox::FixedSize { _parsed_header, size, number_of_samples } => {
                (None, u32::from(*size), *number_of_samples)
            }
            StszBox::VariableSize { _parsed_header, _not_fixed_size, entries } => {
                let variable_iter = entries.entries().map(|entry| entry.get());
                (Some(variable_iter), 0, entries.entry_count())
            }
        };

        // Handrolled "Either" here:
        (0..count).map(move |_| {
            variable_iter
                .as_mut()
                .map_or(Ok(fixed_size), |iter| iter.next().expect("matches count"))
        })
    }
}
