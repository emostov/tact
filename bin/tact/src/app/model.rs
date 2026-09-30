//! Tact's supported model roster and input parsing.

use nanocodex::{Model, ReasoningMode, Thinking};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

/// A model Tact can run, together with the provider that serves it.
///
/// OpenAI models run through the Responses backend. Claude models run through the Messages
/// backend and require an `[anthropic]` table in the configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentModel {
    OpenAi(Model),
    Claude(ClaudeModel),
}

/// Claude models Tact supports.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ClaudeModel {
    Opus55,
}

pub(crate) const SUPPORTED_MODELS: [AgentModel; 4] = [
    AgentModel::OpenAi(Model::Luna),
    AgentModel::OpenAi(Model::Sol),
    AgentModel::OpenAi(Model::Astra),
    AgentModel::Claude(ClaudeModel::Opus55),
];

pub(crate) const DEFAULT_MODEL: AgentModel = AgentModel::OpenAi(Model::Sol);

pub(crate) fn parse(value: &str) -> Result<AgentModel, String> {
    match value {
        "gpt-6-luna" | "luna" => Ok(AgentModel::OpenAi(Model::Luna)),
        "gpt-6.1-sol" | "sol" => Ok(AgentModel::OpenAi(Model::Sol)),
        "gpt-6-astra" | "astra" => Ok(AgentModel::OpenAi(Model::Astra)),
        "claude-opus-5-5" | "opus" => Ok(AgentModel::Claude(ClaudeModel::Opus55)),
        _ => Err(format!(
            "invalid model {value:?}; expected gpt-6-luna, gpt-6.1-sol, gpt-6-astra, or \
             claude-opus-5-5"
        )),
    }
}

pub(crate) fn deserialize_optional<'de, D>(deserializer: D) -> Result<Option<AgentModel>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(|value| parse(&value).map_err(de::Error::custom))
        .transpose()
}

pub(crate) const fn name(model: AgentModel) -> &'static str {
    match model {
        AgentModel::OpenAi(Model::Luna) => "Luna",
        AgentModel::OpenAi(Model::Sol) => "Sol",
        AgentModel::OpenAi(Model::Astra) => "Astra",
        AgentModel::OpenAi(model) => model.as_str(),
        AgentModel::Claude(ClaudeModel::Opus55) => "Opus",
    }
}

impl AgentModel {
    /// The provider-native model identifier.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi(model) => model.as_str(),
            Self::Claude(model) => model.as_str(),
        }
    }

    pub(crate) const fn default_thinking(self) -> Thinking {
        match self {
            Self::OpenAi(model) => model.default_thinking(),
            Self::Claude(_) => Thinking::High,
        }
    }

    /// Claude has no Pro reasoning mode; only Standard is supported.
    pub(crate) fn supports_reasoning_mode(self, mode: ReasoningMode) -> bool {
        match self {
            Self::OpenAi(model) => model.supports_reasoning_mode(mode),
            Self::Claude(_) => mode == ReasoningMode::Standard,
        }
    }
}

impl ClaudeModel {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Opus55 => "claude-opus-5-5",
        }
    }
}

impl From<Model> for AgentModel {
    fn from(model: Model) -> Self {
        Self::OpenAi(model)
    }
}

impl fmt::Display for AgentModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for AgentModel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::OpenAi(model) => model.serialize(serializer),
            Self::Claude(model) => serializer.serialize_str(model.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentModel, ClaudeModel, parse};
    use nanocodex::Model;

    #[test]
    fn accepts_current_model_ids_and_short_names() {
        for (value, expected) in [
            ("gpt-6-luna", AgentModel::OpenAi(Model::Luna)),
            ("luna", AgentModel::OpenAi(Model::Luna)),
            ("gpt-6.1-sol", AgentModel::OpenAi(Model::Sol)),
            ("sol", AgentModel::OpenAi(Model::Sol)),
            ("gpt-6-astra", AgentModel::OpenAi(Model::Astra)),
            ("astra", AgentModel::OpenAi(Model::Astra)),
            ("claude-opus-5-5", AgentModel::Claude(ClaudeModel::Opus55)),
            ("opus", AgentModel::Claude(ClaudeModel::Opus55)),
        ] {
            assert_eq!(parse(value), Ok(expected));
        }
    }

    #[test]
    fn model_ids_round_trip_through_parse() {
        for model in super::SUPPORTED_MODELS {
            assert_eq!(parse(model.as_str()), Ok(model));
        }
    }

    #[test]
    fn rejects_retired_model_ids() {
        for value in [
            "gpt-6-sol",
            "gpt-5.6-luna",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "terra",
            "claude-sonnet-5-5",
        ] {
            assert!(parse(value).is_err(), "retired model {value} was accepted");
        }
    }
}
