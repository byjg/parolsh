//! Questions the agent asks the user, as one form model.
//!
//! Two sources: the standard ACP `elicitation/create` request (Claude, Codex,
//! MCP servers), and Qwen Code's questions, which arrive as a permission
//! request and expect a non-standard `answers` field in the reply.

use agent_client_protocol::schema::v1::{
    ElicitationContentValue, ElicitationPropertySchema, ElicitationSchema, EnumOption, Meta,
    MultiSelectItems,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    /// Shown before the fields; may be empty.
    pub message: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// Where the answer goes in the reply.
    pub key: String,
    pub title: String,
    pub description: Option<String>,
    pub kind: FieldKind,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text,
    Number {
        integer: bool,
    },
    Boolean,
    Choice {
        options: Vec<Choice>,
        multiple: bool,
        /// A free-text answer is accepted too.
        other: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// Sent back to the agent.
    pub value: String,
    /// Shown to the user.
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Text(String),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    Many(Vec<String>),
}

/// Answered fields by key. Skipped fields are absent.
pub type Answers = BTreeMap<String, Answer>;

impl Form {
    /// The form of a standard `elicitation/create` request.
    pub fn from_elicitation(message: String, schema: &ElicitationSchema) -> Self {
        let required = schema.required.clone().unwrap_or_default();
        let fields = schema
            .properties
            .iter()
            .filter_map(|(key, property)| {
                field_from_property(key, property, required.contains(key))
            })
            .collect();
        Self { message, fields }
    }

    /// Qwen's questions: `_meta.qwenQuestions` of a permission request marked
    /// `qwenInteractionKind: "user_question"`. `None` for any other request.
    pub fn from_qwen(meta: Option<&Meta>) -> Option<Self> {
        let meta = meta?;
        if meta.get("qwenInteractionKind")?.as_str()? != "user_question" {
            return None;
        }
        let fields = meta
            .get("qwenQuestions")?
            .as_array()?
            .iter()
            .enumerate()
            .map(|(index, question)| {
                let text = |key: &str| question.get(key).and_then(Value::as_str);
                let options = question
                    .get("options")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|option| {
                        let label = option.get("label")?.as_str()?.to_string();
                        let description = option
                            .get("description")
                            .and_then(Value::as_str)
                            .filter(|description| *description != label)
                            .map(str::to_string);
                        Some(Choice {
                            value: label.clone(),
                            label,
                            description,
                        })
                    })
                    .collect();
                Field {
                    key: index.to_string(),
                    title: text("question").unwrap_or_default().to_string(),
                    description: text("header").map(str::to_string),
                    kind: FieldKind::Choice {
                        options,
                        multiple: question
                            .get("multiSelect")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        other: true,
                    },
                    required: false,
                }
            })
            .collect();
        Some(Self {
            message: String::new(),
            fields,
        })
    }

    /// True when a required field has no answer.
    pub fn missing_required(&self, answers: &Answers) -> bool {
        self.fields
            .iter()
            .any(|field| field.required && !answers.contains_key(&field.key))
    }
}

fn field_from_property(
    key: &str,
    property: &ElicitationPropertySchema,
    required: bool,
) -> Option<Field> {
    let field = |title: &Option<String>, description: &Option<String>, kind| Field {
        key: key.to_string(),
        title: title.clone().unwrap_or_else(|| key.to_string()),
        description: description.clone(),
        kind,
        required,
    };
    Some(match property {
        ElicitationPropertySchema::String(string) => {
            let options: Vec<Choice> = match (&string.one_of, &string.enum_values) {
                (Some(options), _) => options.iter().map(choice).collect(),
                (None, Some(values)) => values.iter().map(|value| plain_choice(value)).collect(),
                (None, None) => Vec::new(),
            };
            let kind = if options.is_empty() {
                FieldKind::Text
            } else {
                FieldKind::Choice {
                    options,
                    multiple: false,
                    other: false,
                }
            };
            field(&string.title, &string.description, kind)
        }
        ElicitationPropertySchema::Number(number) => field(
            &number.title,
            &number.description,
            FieldKind::Number { integer: false },
        ),
        ElicitationPropertySchema::Integer(integer) => field(
            &integer.title,
            &integer.description,
            FieldKind::Number { integer: true },
        ),
        ElicitationPropertySchema::Boolean(boolean) => {
            field(&boolean.title, &boolean.description, FieldKind::Boolean)
        }
        ElicitationPropertySchema::Array(array) => {
            let options = match &array.items {
                MultiSelectItems::Titled(items) => items.options.iter().map(choice).collect(),
                MultiSelectItems::String(items) => items
                    .values
                    .iter()
                    .map(|value| plain_choice(value))
                    .collect(),
                _ => return None,
            };
            field(
                &array.title,
                &array.description,
                FieldKind::Choice {
                    options,
                    multiple: true,
                    other: false,
                },
            )
        }
        _ => return None,
    })
}

fn choice(option: &EnumOption) -> Choice {
    Choice {
        value: option.value.clone(),
        label: option.title.clone(),
        description: option.description.clone(),
    }
}

fn plain_choice(value: &str) -> Choice {
    Choice {
        value: value.to_string(),
        label: value.to_string(),
        description: None,
    }
}

/// Reads one typed answer. Empty input skips the field (`Ok(None)`).
pub fn parse(kind: &FieldKind, input: &str) -> Result<Option<Answer>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    match kind {
        FieldKind::Text => Ok(Some(Answer::Text(input.to_string()))),
        FieldKind::Number { integer: true } => input
            .parse()
            .map(|n| Some(Answer::Integer(n)))
            .map_err(|_| "type a whole number".to_string()),
        FieldKind::Number { integer: false } => input
            .parse()
            .map(|n| Some(Answer::Number(n)))
            .map_err(|_| "type a number".to_string()),
        FieldKind::Boolean => match input.to_lowercase().as_str() {
            "y" | "yes" | "true" => Ok(Some(Answer::Boolean(true))),
            "n" | "no" | "false" => Ok(Some(Answer::Boolean(false))),
            _ => Err("type y or n".to_string()),
        },
        FieldKind::Choice {
            options,
            multiple,
            other,
        } => {
            let pick = |part: &str| {
                part.parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|i| options.get(i))
                    .map(|option| option.value.clone())
            };
            let parts: Vec<&str> = if *multiple {
                input
                    .split(',')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .collect()
            } else {
                vec![input]
            };
            let picked: Option<Vec<String>> = parts.iter().map(|part| pick(part)).collect();
            match (picked, multiple, other) {
                (Some(values), true, _) => Ok(Some(Answer::Many(values))),
                (Some(mut values), false, _) => Ok(Some(Answer::Text(values.remove(0)))),
                (None, _, true) => Ok(Some(Answer::Text(input.to_string()))),
                (None, true, false) => Err(format!(
                    "type numbers from 1 to {}, separated by commas",
                    options.len()
                )),
                (None, false, false) => Err(format!("type a number from 1 to {}", options.len())),
            }
        }
    }
}

/// Answers as the `content` of an accepted elicitation.
pub fn elicitation_content(answers: Answers) -> BTreeMap<String, ElicitationContentValue> {
    answers
        .into_iter()
        .map(|(key, answer)| {
            let value = match answer {
                Answer::Text(text) => ElicitationContentValue::String(text),
                Answer::Integer(n) => ElicitationContentValue::Integer(n),
                Answer::Number(n) => ElicitationContentValue::Number(n),
                Answer::Boolean(b) => ElicitationContentValue::Boolean(b),
                Answer::Many(values) => ElicitationContentValue::StringArray(values),
            };
            (key, value)
        })
        .collect()
}

/// Answers as Qwen's `answers` field: question index to text, several
/// choices joined with ", " as Qwen's own dialog does.
pub fn qwen_answers(answers: Answers) -> Value {
    let map: serde_json::Map<String, Value> = answers
        .into_iter()
        .map(|(key, answer)| {
            let text = match answer {
                Answer::Text(text) => text,
                Answer::Many(values) => values.join(", "),
                Answer::Integer(n) => n.to_string(),
                Answer::Number(n) => n.to_string(),
                Answer::Boolean(b) => b.to_string(),
            };
            (key, json!(text))
        })
        .collect();
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice_field(multiple: bool, other: bool) -> FieldKind {
        FieldKind::Choice {
            options: vec![
                plain_choice("Red"),
                plain_choice("Blue"),
                plain_choice("Green"),
            ],
            multiple,
            other,
        }
    }

    #[test]
    fn reads_a_standard_elicitation_schema() {
        let schema: ElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "question_0": {
                    "type": "string",
                    "title": "Which color?",
                    "oneOf": [
                        {"const": "red", "title": "Red"},
                        {"const": "blue", "title": "Blue", "description": "The sea"}
                    ]
                },
                "question_1": {
                    "type": "array",
                    "title": "Which sizes?",
                    "items": {"anyOf": [{"const": "s", "title": "S"}, {"const": "m", "title": "M"}]}
                },
                "note": {"type": "string", "description": "Anything else"},
                "agree": {"type": "boolean", "title": "Agree?"}
            },
            "required": ["question_0"]
        }))
        .unwrap();

        let form = Form::from_elicitation("Pick".to_string(), &schema);

        let kinds: Vec<(&str, &FieldKind, bool)> = form
            .fields
            .iter()
            .map(|f| (f.key.as_str(), &f.kind, f.required))
            .collect();
        assert_eq!(form.message, "Pick");
        assert_eq!(
            kinds,
            [
                ("agree", &FieldKind::Boolean, false),
                ("note", &FieldKind::Text, false),
                (
                    "question_0",
                    &FieldKind::Choice {
                        options: vec![
                            plain_choice("red").with_label("Red"),
                            Choice {
                                value: "blue".to_string(),
                                label: "Blue".to_string(),
                                description: Some("The sea".to_string()),
                            },
                        ],
                        multiple: false,
                        other: false,
                    },
                    true
                ),
                (
                    "question_1",
                    &FieldKind::Choice {
                        options: vec![
                            plain_choice("s").with_label("S"),
                            plain_choice("m").with_label("M"),
                        ],
                        multiple: true,
                        other: false,
                    },
                    false
                ),
            ]
        );
        assert_eq!(form.fields[1].title, "note");
        assert_eq!(form.fields[1].description.as_deref(), Some("Anything else"));
    }

    #[test]
    fn reads_qwen_questions_from_the_permission_meta() {
        let meta: Meta = serde_json::from_value(json!({
            "qwenInteractionKind": "user_question",
            "qwenQuestions": [{
                "question": "Which color do you prefer?",
                "header": "Color preference",
                "options": [
                    {"label": "Red", "description": "Red"},
                    {"label": "Blue", "description": "Go with blue"}
                ]
            }]
        }))
        .unwrap();

        let form = Form::from_qwen(Some(&meta)).unwrap();

        assert_eq!(
            form.fields,
            [Field {
                key: "0".to_string(),
                title: "Which color do you prefer?".to_string(),
                description: Some("Color preference".to_string()),
                kind: FieldKind::Choice {
                    options: vec![
                        plain_choice("Red"),
                        Choice {
                            value: "Blue".to_string(),
                            label: "Blue".to_string(),
                            description: Some("Go with blue".to_string()),
                        },
                    ],
                    multiple: false,
                    other: true,
                },
                required: false,
            }]
        );
    }

    #[test]
    fn other_permission_requests_are_not_qwen_questions() {
        let meta: Meta = serde_json::from_value(json!({"toolName": "write_file"})).unwrap();

        assert_eq!(Form::from_qwen(Some(&meta)), None);
        assert_eq!(Form::from_qwen(None), None);
    }

    #[test]
    fn parses_choices_by_number() {
        let single = choice_field(false, false);
        let multiple = choice_field(true, false);

        assert_eq!(parse(&single, "2"), Ok(Some(Answer::Text("Blue".into()))));
        assert_eq!(
            parse(&multiple, "1, 3"),
            Ok(Some(Answer::Many(vec!["Red".into(), "Green".into()])))
        );
        assert!(parse(&single, "9").is_err());
        assert!(parse(&single, "purple").is_err());
    }

    #[test]
    fn a_free_text_answer_when_other_is_allowed() {
        let kind = choice_field(false, true);

        assert_eq!(
            parse(&kind, "purple"),
            Ok(Some(Answer::Text("purple".into())))
        );
        assert_eq!(parse(&kind, "1"), Ok(Some(Answer::Text("Red".into()))));
    }

    #[test]
    fn parses_text_numbers_and_booleans() {
        assert_eq!(
            parse(&FieldKind::Text, " hi "),
            Ok(Some(Answer::Text("hi".into())))
        );
        assert_eq!(
            parse(&FieldKind::Number { integer: true }, "42"),
            Ok(Some(Answer::Integer(42)))
        );
        assert!(parse(&FieldKind::Number { integer: true }, "4.2").is_err());
        assert_eq!(
            parse(&FieldKind::Boolean, "Yes"),
            Ok(Some(Answer::Boolean(true)))
        );
        assert!(parse(&FieldKind::Boolean, "maybe").is_err());
    }

    #[test]
    fn empty_input_skips() {
        assert_eq!(parse(&FieldKind::Text, "  "), Ok(None));
        assert_eq!(parse(&choice_field(true, false), ""), Ok(None));
    }

    #[test]
    fn answers_become_elicitation_content_and_qwen_answers() {
        let answers = Answers::from([
            ("0".to_string(), Answer::Text("Blue".into())),
            ("1".to_string(), Answer::Many(vec!["A".into(), "B".into()])),
        ]);

        assert_eq!(
            qwen_answers(answers.clone()),
            json!({"0": "Blue", "1": "A, B"})
        );
        assert_eq!(
            serde_json::to_value(elicitation_content(answers)).unwrap(),
            json!({"0": "Blue", "1": ["A", "B"]})
        );
    }

    #[test]
    fn a_skipped_required_field_is_missing() {
        let form = Form {
            message: String::new(),
            fields: vec![Field {
                key: "q".to_string(),
                title: "Q".to_string(),
                description: None,
                kind: FieldKind::Text,
                required: true,
            }],
        };

        assert!(form.missing_required(&Answers::new()));
        assert!(!form.missing_required(&Answers::from([(
            "q".to_string(),
            Answer::Text("x".into())
        )])));
    }

    impl Choice {
        fn with_label(mut self, label: &str) -> Self {
            self.label = label.to_string();
            self
        }
    }
}
