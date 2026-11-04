#[derive(Debug, Clone, PartialEq)]
pub(super) enum Operand {
    KeyPath(String),
    String(String),
    Number(f64),
    Bool(bool),
    Argument { index: usize, kind: char },
    Variable(String),
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Comparison {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Expression {
    Compare(Operand, Comparison, Operand),
    And(Box<Expression>, Box<Expression>),
    Or(Box<Expression>, Box<Expression>),
    Not(Box<Expression>),
    Constant(bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comparisons_and_compound_precedence() {
        assert_eq!(
            parse("score >= 10").unwrap(),
            Expression::Compare(
                Operand::KeyPath("score".into()),
                Comparison::GreaterOrEqual,
                Operand::Number(10.0)
            )
        );
        assert!(matches!(
            parse("score > 10 AND (enabled == YES OR name == 'guest')").unwrap(),
            Expression::And(_, _)
        ));
        assert_eq!(
            parse("enabled == YES").unwrap(),
            Expression::Compare(
                Operand::KeyPath("enabled".into()),
                Comparison::Equal,
                Operand::Bool(true),
            )
        );
    }

    #[test]
    fn parses_and_evaluates_object_substitution() {
        let expression = parse("name == %@").unwrap();
        assert!(expression
            .evaluate(
                |key| (key == "name").then_some(Operand::String("four".into())),
                &[Operand::String("four".into())],
            )
            .unwrap());
    }

    #[test]
    fn object_placeholder_accepts_number_and_rejects_wrong_type() {
        let expression = parse("score == %@").unwrap();
        assert!(expression
            .evaluate(
                |key| (key == "score").then_some(Operand::Number(4.0)),
                &[Operand::Number(4.0)],
            )
            .unwrap());
        assert!(expression
            .evaluate(
                |key| (key == "score").then_some(Operand::Number(4.0)),
                &[Operand::String("4".into())],
            )
            .is_err());
    }

    #[test]
    fn supports_escaped_quotes_and_not() {
        assert_eq!(
            parse("NOT name == 'a\\'b'").unwrap(),
            Expression::Not(Box::new(Expression::Compare(
                Operand::KeyPath("name".into()),
                Comparison::Equal,
                Operand::String("a'b".into())
            )))
        );
    }

    #[test]
    fn rejects_unsupported_or_malformed_syntax() {
        assert!(parse("name CONTAINS 'x'").is_err());
        assert!(parse("score ==").is_err());
        assert!(parse("name == 'unterminated").is_err());
    }

    #[test]
    fn bare_right_hand_words_are_not_string_literals() {
        let expression = parse("name == guest").unwrap();
        assert_eq!(
            expression,
            Expression::Compare(
                Operand::KeyPath("name".into()),
                Comparison::Equal,
                Operand::KeyPath("guest".into()),
            )
        );
    }

    #[test]
    fn string_ordering_is_lexicographic() {
        let expression = parse("name < 'b'").unwrap();
        assert!(expression
            .evaluate(
                |key| (key == "name").then_some(Operand::String("a".into())),
                &[]
            )
            .unwrap());
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    String(String),
    Number(f64),
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Argument { index: usize, kind: char },
    LParen,
    RParen,
}

pub(super) fn parse(format: &str) -> Result<Expression, String> {
    parse_with_arguments(format)
}

pub(super) fn parse_with_arguments(format: &str) -> Result<Expression, String> {
    let tokens = tokenize(format)?;
    let mut parser = Parser { tokens, index: 0 };
    let expression = parser.parse_or()?;
    if parser.index != parser.tokens.len() {
        return Err("unexpected trailing token".into());
    }
    Ok(expression)
}

#[cfg(test)]
mod evaluation_tests {
    use super::*;

    #[test]
    fn comparison_operators_observe_operand_order() {
        let expression = parse("score > 3").unwrap();
        assert!(expression
            .evaluate(|_| Some(Operand::Number(5.0)), &[])
            .unwrap());
        assert!(!expression
            .evaluate(|_| Some(Operand::Number(2.0)), &[])
            .unwrap());
    }

    #[test]
    fn positional_placeholders_keep_their_argument_index() {
        let expression = parse("score > %d AND rank < %d").unwrap();
        let score_and_rank = [Operand::Number(7.0), Operand::Number(2.0)];
        assert!(expression
            .evaluate(
                |key| match key {
                    "score" => Some(Operand::Number(8.0)),
                    "rank" => Some(Operand::Number(1.0)),
                    _ => None,
                },
                &score_and_rank,
            )
            .unwrap());
        let swapped = [Operand::Number(2.0), Operand::Number(7.0)];
        assert!(!expression
            .evaluate(
                |key| match key {
                    "score" => Some(Operand::Number(1.0)),
                    "rank" => Some(Operand::Number(8.0)),
                    _ => None,
                },
                &swapped,
            )
            .unwrap());
    }

    #[test]
    fn positional_substitutions_are_numbered_in_source_order() {
        let expression = parse("score > %d AND rank < %d").unwrap();
        assert!(expression
            .evaluate(
                |key| match key {
                    "score" => Some(Operand::Number(5.0)),
                    "rank" => Some(Operand::Number(3.0)),
                    _ => None,
                },
                &[Operand::Number(4.0), Operand::Number(4.0)],
            )
            .unwrap());
        assert!(!expression
            .evaluate(
                |key| match key {
                    "score" => Some(Operand::Number(5.0)),
                    "rank" => Some(Operand::Number(3.0)),
                    _ => None,
                },
                &[Operand::Number(4.0), Operand::Number(2.0)],
            )
            .unwrap());
    }

    #[test]
    fn evaluates_comparisons_boolean_operators_and_substitutions() {
        let expression = parse("score >= 10 AND (enabled == YES OR name == %@)").unwrap();
        assert!(expression
            .evaluate(
                |key| match key {
                    "score" => Some(Operand::Number(12.0)),
                    "enabled" => Some(Operand::Bool(false)),
                    "name" => Some(Operand::String("player".into())),
                    _ => None,
                },
                &[Operand::String("player".into())]
            )
            .unwrap());
    }

    #[test]
    fn substitutions_are_parsed_as_arguments() {
        let expression = parse("name == %@ AND score > %d").unwrap();
        assert!(expression
            .evaluate(
                |key| match key {
                    "name" => Some(Operand::String("Ada".into())),
                    "score" => Some(Operand::Number(8.0)),
                    _ => None,
                },
                &[Operand::String("Ada".into()), Operand::Number(7.0)]
            )
            .unwrap());
    }

    #[test]
    fn evaluates_resolved_values_and_compound_predicates() {
        let expression = parse("score >= 10 AND NOT active == NO").unwrap();
        let values = [
            ("score", Operand::Number(12.0)),
            ("active", Operand::Bool(true)),
        ];
        assert!(expression
            .evaluate(
                |key| values
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| value.clone()),
                &[]
            )
            .unwrap());
        assert!(!expression
            .evaluate(
                |key| match key {
                    "score" => Some(Operand::Number(9.0)),
                    "active" => Some(Operand::Bool(true)),
                    _ => None,
                },
                &[]
            )
            .unwrap());
    }

    #[test]
    fn reports_missing_values_and_mismatched_types() {
        let expression = parse("score > 1").unwrap();
        assert!(expression.evaluate(|_| None, &[]).is_err());
        assert!(expression
            .evaluate(|_| Some(Operand::String("high".into())), &[])
            .is_err());
    }

    #[test]
    fn format_arguments_keep_their_sequential_indices() {
        let expression = parse_with_arguments("name == %@ AND score > %d").unwrap();
        assert!(expression
            .evaluate(
                |key| match key {
                    "name" => Some(Operand::String("Ada".into())),
                    "score" => Some(Operand::Number(8.0)),
                    _ => None,
                },
                &[Operand::String("Ada".into()), Operand::Number(7.0)]
            )
            .unwrap());
    }

    #[test]
    fn second_substitution_uses_second_argument() {
        let expression = parse("score > %d AND rank < %d").unwrap();
        let values = [Operand::Number(1.0), Operand::Number(4.0)];
        assert!(expression
            .evaluate(
                |key| match key {
                    "score" => Some(Operand::Number(8.0)),
                    "rank" => Some(Operand::Number(2.0)),
                    _ => None,
                },
                &values
            )
            .unwrap());
        let values = [Operand::Number(1.0), Operand::Number(1.0)];
        assert!(!expression
            .evaluate(
                |key| match key {
                    "score" => Some(Operand::Number(8.0)),
                    "rank" => Some(Operand::Number(2.0)),
                    _ => None,
                },
                &values
            )
            .unwrap());
    }

    #[test]
    fn substitution_types_are_checked() {
        let expression = parse("name == %@ AND score > %d").unwrap();
        assert!(expression
            .evaluate(
                |key| match key {
                    "name" => Some(Operand::String("Ada".into())),
                    "score" => Some(Operand::Number(8.0)),
                    _ => None,
                },
                &[Operand::Number(7.0), Operand::String("Ada".into())]
            )
            .is_err());
    }

    #[test]
    fn substitutes_dollar_variables_in_key_paths() {
        let expression = parse("$threshold < score")
            .unwrap()
            .substitute(&[("$threshold".into(), Operand::Number(7.0))]);
        assert!(expression
            .evaluate(|key| (key == "score").then_some(Operand::Number(8.0)), &[])
            .unwrap());
    }

    #[test]
    fn later_substitution_variables_override_earlier_values() {
        let expression = parse("$minimum < score").unwrap().substitute(&[
            ("$minimum".into(), Operand::Number(10.0)),
            ("$minimum".into(), Operand::Number(5.0)),
        ]);
        assert!(expression
            .evaluate(|key| (key == "score").then_some(Operand::Number(8.0)), &[])
            .unwrap());
    }

    #[test]
    fn object_substitutions_accept_supported_value_types() {
        let expression = parse("value == %@").unwrap();
        assert!(expression
            .evaluate(
                |key| (key == "value").then_some(Operand::Number(4.0)),
                &[Operand::Number(4.0)],
            )
            .unwrap());
    }

    #[test]
    fn dollar_variable_substitutions_override_stored_values() {
        let expression = parse("$minimum < score").unwrap();
        let stored = [("$minimum".into(), Operand::Number(10.0))];
        let call = [("$minimum".into(), Operand::Number(5.0))];
        let merged: Vec<_> = stored.into_iter().chain(call).collect();
        let expression = expression.substitute(&merged);
        assert!(expression
            .evaluate(|key| (key == "score").then_some(Operand::Number(8.0)), &[])
            .unwrap());
    }

    #[test]
    fn object_substitution_accepts_number_for_at_placeholder() {
        let expression = parse("score == %@").unwrap();
        assert!(expression
            .evaluate(
                |key| (key == "score").then_some(Operand::Number(3.0)),
                &[Operand::Number(3.0)],
            )
            .unwrap());
    }

    #[test]
    fn at_placeholder_accepts_string_argument() {
        let expression = parse("name == %@").unwrap();
        assert!(expression
            .evaluate(
                |key| (key == "name").then_some(Operand::String("x".into())),
                &[Operand::String("x".into())],
            )
            .unwrap());
    }

    #[test]
    fn comparison_supports_null_and_orders() {
        for (format, expected) in [
            ("name == NIL", true),
            ("score <= 3", true),
            ("score != 4", true),
        ] {
            let expression = parse(format).unwrap();
            assert_eq!(
                expression
                    .evaluate(
                        |key| match key {
                            "name" => Some(Operand::Null),
                            "score" => Some(Operand::Number(3.0)),
                            _ => None,
                        },
                        &[]
                    )
                    .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn null_inequality_matches_non_null_values() {
        let expression = parse("name != NIL").unwrap();
        assert!(expression
            .evaluate(
                |key| (key == "name").then_some(Operand::String("Ada".into())),
                &[],
            )
            .unwrap());
    }

    #[test]
    fn parses_true_and_false_predicate_keywords() {
        assert_eq!(parse("TRUEPREDICATE"), Ok(Expression::Constant(true)));
        assert_eq!(parse("FALSEPREDICATE"), Ok(Expression::Constant(false)));
        assert!(matches!(
            parse("YES == YES"),
            Ok(Expression::Compare(_, _, _))
        ));
        assert!(matches!(
            parse("YES AND score > 0"),
            Ok(Expression::And(_, _))
        ));
    }

    #[test]
    fn parses_dotted_substitution_variables() {
        assert!(matches!(
            parse("$user.name == 'Ada'").unwrap(),
            Expression::Compare(Operand::Variable(name), _, _)
                if name == "$user.name"
        ));
    }

    #[test]
    fn rejects_equality_between_missing_keypath_and_null_literal() {
        let expression = parse("missing == NIL").unwrap();
        assert!(expression.evaluate(|_| None, &[]).is_err());
    }

    #[test]
    fn comparison_supports_keypath_on_right_side() {
        let expression = parse("minimum < score").unwrap();
        assert!(expression
            .evaluate(
                |key| match key {
                    "minimum" => Some(Operand::Number(3.0)),
                    "score" => Some(Operand::Number(5.0)),
                    _ => None,
                },
                &[],
            )
            .unwrap());
    }

    #[test]
    fn named_variable_values_are_replaced_before_comparison() {
        let expression = parse("$minimum < score").unwrap();
        let expression = expression.substitute(&[("$minimum".into(), Operand::Number(3.0))]);
        assert!(expression
            .evaluate(|key| (key == "score").then_some(Operand::Number(5.0)), &[])
            .unwrap());
    }
}

pub(super) fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut chars = input.chars().peekable();
    let mut tokens = Vec::new();
    let mut argument_index = 0;
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        if ch == '$' {
            let mut value = String::from(ch);
            while chars
                .peek()
                .is_some_and(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
            {
                value.push(chars.next().unwrap());
            }
            if value.len() == 1 {
                return Err("empty variable name".into());
            }
            tokens.push(Token::Word(value));
            continue;
        }
        match ch {
            '(' => tokens.push(Token::LParen),
            ')' => tokens.push(Token::RParen),
            '=' => {
                if chars.next() != Some('=') {
                    return Err("expected ==".into());
                }
                tokens.push(Token::Eq);
            }
            '!' => match chars.next() {
                Some('=') => tokens.push(Token::Ne),
                _ => return Err("expected !=".into()),
            },
            '<' | '>' => {
                let equal = chars.peek() == Some(&'=');
                if equal {
                    chars.next();
                }
                tokens.push(match (ch, equal) {
                    ('<', true) => Token::Le,
                    ('<', false) => Token::Lt,
                    ('>', true) => Token::Ge,
                    ('>', false) => Token::Gt,
                    _ => unreachable!(),
                });
            }
            '\'' | '"' => {
                let mut value = String::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '\\' {
                        value.push(chars.next().ok_or("unterminated escape")?);
                    } else if c == ch {
                        closed = true;
                        break;
                    } else {
                        value.push(c);
                    }
                }
                if !closed {
                    return Err("unterminated string".into());
                }
                tokens.push(Token::String(value));
            }
            '%' => match chars.next() {
                Some(kind @ ('@' | 'd' | 'i' | 'f' | 's')) => {
                    tokens.push(Token::Argument {
                        index: argument_index,
                        kind,
                    });
                    argument_index += 1;
                }
                _ => return Err("unsupported format substitution".into()),
            },
            _ if ch.is_ascii_digit()
                || (ch == '-' && chars.peek().is_some_and(char::is_ascii_digit)) =>
            {
                let mut value = String::from(ch);
                while chars
                    .peek()
                    .is_some_and(|c| c.is_ascii_digit() || *c == '.')
                {
                    value.push(chars.next().unwrap());
                }
                tokens.push(Token::Number(value.parse().map_err(|_| "invalid number")?));
            }
            _ if ch.is_alphabetic() || ch == '_' || ch == '$' => {
                let mut word = String::from(ch);
                while chars
                    .peek()
                    .is_some_and(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
                {
                    word.push(chars.next().unwrap());
                }
                tokens.push(Token::Word(word));
            }
            _ => return Err(format!("unsupported character {ch:?}")),
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
}

impl Parser {
    fn peek_word(&self, word: &str) -> bool {
        matches!(self.tokens.get(self.index), Some(Token::Word(value)) if value.eq_ignore_ascii_case(word))
    }

    fn take_word(&mut self, word: &str) -> bool {
        if self.peek_word(word) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn next_token_is_comparison(&self) -> bool {
        matches!(
            self.tokens.get(self.index + 1),
            Some(Token::Eq | Token::Ne | Token::Lt | Token::Le | Token::Gt | Token::Ge)
        )
    }

    fn parse_or(&mut self) -> Result<Expression, String> {
        let mut expression = self.parse_and()?;
        while self.take_word("OR") {
            expression = Expression::Or(Box::new(expression), Box::new(self.parse_and()?));
        }
        Ok(expression)
    }

    fn parse_and(&mut self) -> Result<Expression, String> {
        let mut expression = self.parse_not()?;
        while self.take_word("AND") {
            expression = Expression::And(Box::new(expression), Box::new(self.parse_not()?));
        }
        Ok(expression)
    }

    fn parse_not(&mut self) -> Result<Expression, String> {
        if self.take_word("NOT") {
            return Ok(Expression::Not(Box::new(self.parse_not()?)));
        }
        if self.take_word("TRUEPREDICATE") {
            return Ok(Expression::Constant(true));
        }
        if self.take_word("FALSEPREDICATE") {
            return Ok(Expression::Constant(false));
        }
        if (self.peek_word("YES") || self.peek_word("TRUE")) && !self.next_token_is_comparison() {
            self.index += 1;
            return Ok(Expression::Constant(true));
        }
        if (self.peek_word("NO") || self.peek_word("FALSE")) && !self.next_token_is_comparison() {
            self.index += 1;
            return Ok(Expression::Constant(false));
        }
        if self.tokens.get(self.index) == Some(&Token::LParen) {
            self.index += 1;
            let nested = self.parse_or()?;
            if self.tokens.get(self.index) != Some(&Token::RParen) {
                return Err("expected )".into());
            }
            self.index += 1;
            return Ok(nested);
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expression, String> {
        let left = self.parse_operand()?;
        let comparison = match self.tokens.get(self.index) {
            Some(Token::Eq) => Comparison::Equal,
            Some(Token::Ne) => Comparison::NotEqual,
            Some(Token::Lt) => Comparison::Less,
            Some(Token::Le) => Comparison::LessOrEqual,
            Some(Token::Gt) => Comparison::Greater,
            Some(Token::Ge) => Comparison::GreaterOrEqual,
            _ => return Err("expected comparison operator".into()),
        };
        self.index += 1;
        Ok(Expression::Compare(left, comparison, self.parse_operand()?))
    }

    fn parse_operand(&mut self) -> Result<Operand, String> {
        let token = self
            .tokens
            .get(self.index)
            .ok_or("expected operand")?
            .clone();
        self.index += 1;
        match token {
            Token::String(value) => Ok(Operand::String(value)),
            Token::Number(value) => Ok(Operand::Number(value)),
            Token::Word(value)
                if value.eq_ignore_ascii_case("YES") || value.eq_ignore_ascii_case("TRUE") =>
            {
                Ok(Operand::Bool(true))
            }
            Token::Word(value)
                if value.eq_ignore_ascii_case("NO") || value.eq_ignore_ascii_case("FALSE") =>
            {
                Ok(Operand::Bool(false))
            }
            Token::Word(value)
                if value.eq_ignore_ascii_case("NIL") || value.eq_ignore_ascii_case("NULL") =>
            {
                Ok(Operand::Null)
            }
            Token::Word(value) if value.starts_with('$') => Ok(Operand::Variable(value)),
            Token::Word(value) => Ok(Operand::KeyPath(value)),
            Token::Argument { index, kind } => Ok(Operand::Argument { index, kind }),
            _ => Err("expected key path or literal".into()),
        }
    }
}

impl Expression {
    pub(super) fn substitute(&self, variables: &[(String, Operand)]) -> Self {
        fn replace(operand: &Operand, variables: &[(String, Operand)]) -> Operand {
            if let Operand::Variable(name) = operand {
                if let Some((_, value)) = variables.iter().rev().find(|(key, _)| key == name) {
                    return value.clone();
                }
            }
            operand.clone()
        }
        match self {
            Expression::Compare(left, comparison, right) => Expression::Compare(
                replace(left, variables),
                comparison.clone(),
                replace(right, variables),
            ),
            Expression::And(left, right) => Expression::And(
                Box::new(left.substitute(variables)),
                Box::new(right.substitute(variables)),
            ),
            Expression::Or(left, right) => Expression::Or(
                Box::new(left.substitute(variables)),
                Box::new(right.substitute(variables)),
            ),
            Expression::Not(inner) => Expression::Not(Box::new(inner.substitute(variables))),
            Expression::Constant(value) => Expression::Constant(*value),
        }
    }

    pub(super) fn evaluate(
        &self,
        mut key_path: impl FnMut(&str) -> Option<Operand>,
        arguments: &[Operand],
    ) -> Result<bool, String> {
        fn resolve(
            operand: &Operand,
            key_path: &mut impl FnMut(&str) -> Option<Operand>,
            arguments: &[Operand],
        ) -> Result<Operand, String> {
            match operand {
                Operand::KeyPath(path) => {
                    key_path(path).ok_or_else(|| format!("unavailable key path {path:?}"))
                }
                Operand::Variable(name) => Err(format!("unresolved predicate variable {name:?}")),
                Operand::Argument { index, kind } => {
                    let value = arguments.get(*index).cloned().ok_or_else(|| {
                        format!("missing predicate substitution argument {index}")
                    })?;
                    match (kind, value) {
                        (
                            '@',
                            value @ (Operand::String(_)
                            | Operand::Number(_)
                            | Operand::Bool(_)
                            | Operand::Null),
                        ) => Ok(value),
                        ('s', value @ (Operand::String(_) | Operand::Null)) => Ok(value),
                        ('d' | 'i' | 'f', value @ Operand::Number(_)) => Ok(value),
                        _ => Err(format!("substitution %{kind} has an incompatible argument")),
                    }
                }
                value => Ok(value.clone()),
            }
        }

        fn compare(left: Operand, op: &Comparison, right: Operand) -> Result<bool, String> {
            if matches!(&left, Operand::Null) || matches!(&right, Operand::Null) {
                return Ok(match op {
                    Comparison::Equal => matches!((&left, &right), (Operand::Null, Operand::Null)),
                    Comparison::NotEqual => {
                        !matches!((&left, &right), (Operand::Null, Operand::Null))
                    }
                    _ => return Err("ordered comparison with NIL is unsupported".into()),
                });
            }
            let ordering = match (&left, &right) {
                (Operand::String(a), Operand::String(b)) => Some(a.cmp(b)),
                (Operand::Number(a), Operand::Number(b)) => a.partial_cmp(b),
                (Operand::Bool(a), Operand::Bool(b)) => Some(a.cmp(b)),
                (Operand::Bool(a), Operand::Number(b)) => (*a as u8 as f64).partial_cmp(b),
                (Operand::Number(a), Operand::Bool(b)) => a.partial_cmp(&(*b as u8 as f64)),
                (Operand::Null, Operand::Null) => Some(std::cmp::Ordering::Equal),
                _ => return Err("predicate comparison operands have incompatible types".into()),
            }
            .ok_or_else(|| "cannot compare NaN predicate numbers".to_string())?;
            Ok(match op {
                Comparison::Equal => ordering.is_eq(),
                Comparison::NotEqual => !ordering.is_eq(),
                Comparison::Less => ordering.is_lt(),
                Comparison::LessOrEqual => !ordering.is_gt(),
                Comparison::Greater => ordering.is_gt(),
                Comparison::GreaterOrEqual => !ordering.is_lt(),
            })
        }

        fn eval(
            expr: &Expression,
            key_path: &mut impl FnMut(&str) -> Option<Operand>,
            arguments: &[Operand],
        ) -> Result<bool, String> {
            match expr {
                Expression::Constant(value) => Ok(*value),
                Expression::Not(inner) => Ok(!eval(inner, key_path, arguments)?),
                Expression::And(a, b) => {
                    Ok(eval(a, key_path, arguments)? && eval(b, key_path, arguments)?)
                }
                Expression::Or(a, b) => {
                    Ok(eval(a, key_path, arguments)? || eval(b, key_path, arguments)?)
                }
                Expression::Compare(left, op, right) => compare(
                    resolve(left, key_path, arguments)?,
                    op,
                    resolve(right, key_path, arguments)?,
                ),
            }
        }

        eval(self, &mut key_path, arguments)
    }
}
