use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// A Range is an iterator over integers.
use crate::{
    ast::{RangeInclusion, RangeOperator},
    *,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Range {
    pub from: Value,
    pub incr: Value,
    pub to: Value,
    pub inclusion: RangeInclusion,
}

impl Range {
    pub fn new(
        expr_span: Span,
        from: Value,
        next: Value,
        to: Value,
        operator: &RangeOperator,
    ) -> Result<Range, ShellError> {
        // Select from & to values if they're not specified
        // TODO: Replace the placeholder values with proper min/max based on data type
        let from = if let Value::Nothing { .. } = from {
            Value::Int {
                val: 0i64,
                span: Span::unknown(),
            }
        } else {
            from
        };

        let to = if let Value::Nothing { .. } = to {
            if let Ok(Value::Bool { val: true, .. }) = next.lt(expr_span, &from) {
                Value::Int {
                    val: -100i64,
                    span: Span::unknown(),
                }
            } else {
                Value::Int {
                    val: 100i64,
                    span: Span::unknown(),
                }
            }
        } else {
            to
        };

        // Check if the range counts up or down
        let moves_up = matches!(from.lte(expr_span, &to), Ok(Value::Bool { val: true, .. }));

        // Convert the next value into the inctement
        let incr = if let Value::Nothing { .. } = next {
            if moves_up {
                Value::Int {
                    val: 1i64,
                    span: Span::unknown(),
                }
            } else {
                Value::Int {
                    val: -1i64,
                    span: Span::unknown(),
                }
            }
        } else {
            next.sub(operator.next_op_span, &from)?
        };

        let zero = Value::Int {
            val: 0i64,
            span: Span::unknown(),
        };

        // Increment must be non-zero, otherwise we iterate forever
        if matches!(incr.eq(expr_span, &zero), Ok(Value::Bool { val: true, .. })) {
            return Err(ShellError::CannotCreateRange(expr_span));
        }

        // If to > from, then incr > 0, otherwise we iterate forever
        if let (Value::Bool { val: true, .. }, Value::Bool { val: false, .. }) = (
            to.gt(operator.span, &from)?,
            incr.gt(operator.next_op_span, &zero)?,
        ) {
            return Err(ShellError::CannotCreateRange(expr_span));
        }

        // If to < from, then incr < 0, otherwise we iterate forever
        if let (Value::Bool { val: true, .. }, Value::Bool { val: false, .. }) = (
            to.lt(operator.span, &from)?,
            incr.lt(operator.next_op_span, &zero)?,
        ) {
            return Err(ShellError::CannotCreateRange(expr_span));
        }

        Ok(Range {
            from,
            incr,
            to,
            inclusion: operator.inclusion,
        })
    }
}

impl IntoIterator for Range {
    type Item = Value;

    type IntoIter = RangeIterator;

    fn into_iter(self) -> Self::IntoIter {
        let span = self.from.span();

        RangeIterator::new(self, span)
    }
}

pub struct RangeIterator {
    curr: Value,
    end: Value,
    span: Span,
    is_end_inclusive: bool,
    moves_up: bool,
    incr: Value,
    done: bool,
}

impl RangeIterator {
    pub fn new(range: Range, span: Span) -> RangeIterator {
        let start = match range.from {
            Value::Nothing { .. } => Value::Int { val: 0, span },
            x => x,
        };

        let end = match range.to {
            Value::Nothing { .. } => Value::Int {
                val: i64::MAX,
                span,
            },
            x => x,
        };

        RangeIterator {
            moves_up: matches!(start.lte(span, &end), Ok(Value::Bool { val: true, .. })),
            curr: start,
            end,
            span,
            is_end_inclusive: matches!(range.inclusion, RangeInclusion::Inclusive),
            done: false,
            incr: range.incr,
        }
    }

    pub fn contains(&self, x: &Value) -> bool {
        let ordering_against_curr = compare_numbers(x, &self.curr);
        let ordering_against_end = compare_numbers(x, &self.end);

        match (ordering_against_curr, ordering_against_end) {
            (Some(Ordering::Greater | Ordering::Equal), Some(Ordering::Less)) if self.moves_up => {
                true
            }
            (Some(Ordering::Less | Ordering::Equal), Some(Ordering::Greater)) if !self.moves_up => {
                true
            }
            (Some(_), Some(Ordering::Equal)) if self.is_end_inclusive => true,
            (_, _) => false,
        }
    }
}

fn compare_numbers(val: &Value, other: &Value) -> Option<Ordering> {
    match (val, other) {
        (Value::Int { val, .. }, Value::Int { val: other, .. }) => Some(val.cmp(other)),
        (Value::Float { val, .. }, Value::Float { val: other, .. }) => compare_floats(*val, *other),
        (Value::Float { val, .. }, Value::Int { val: other, .. }) => {
            compare_floats(*val, *other as f64)
        }
        (Value::Int { val, .. }, Value::Float { val: other, .. }) => {
            compare_floats(*val as f64, *other)
        }
        _ => None,
    }
}

// Compare two floating point numbers. The decision interval for equality is dynamically scaled
// as the value being compared increases in magnitude.
fn compare_floats(val: f64, other: f64) -> Option<Ordering> {
    let prec = f64::EPSILON.max(val.abs() * f64::EPSILON);

    if (other - val).abs() < prec {
        return Some(Ordering::Equal);
    }

    val.partial_cmp(&other)
}

impl Iterator for RangeIterator {
    type Item = Value;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        let ordering = if matches!(self.end, Value::Nothing { .. }) {
            Some(Ordering::Less)
        } else {
            compare_numbers(&self.curr, &self.end)
        };

        let ordering = if let Some(ord) = ordering {
            ord
        } else {
            self.done = true;
            return Some(Value::Error {
                error: ShellError::CannotCreateRange(self.span),
            });
        };

        let desired_ordering = if self.moves_up {
            Ordering::Less
        } else {
            Ordering::Greater
        };

        if (ordering == desired_ordering) || (self.is_end_inclusive && ordering == Ordering::Equal)
        {
            let next_value = self.curr.add(self.span, &self.incr);

            let mut next = match next_value {
                Ok(result) => result,

                Err(error) => {
                    self.done = true;
                    return Some(Value::Error { error });
                }
            };
            std::mem::swap(&mut self.curr, &mut next);

            Some(next)
        } else {
            None
        }
    }
}
