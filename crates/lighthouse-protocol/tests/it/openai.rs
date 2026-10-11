use async_openai::types::{
    finetuning::{
        CreateFineTuningJobRequest as OpenAiCreateFineTuningJobRequest,
        FineTuneJobError as OpenAiFineTuneJobError, FineTuneMethod as OpenAiFineTuneMethod,
        FineTuneSupervisedHyperparameters as OpenAiFineTuneSupervisedHyperparameters,
        FineTuneSupervisedMethod as OpenAiFineTuneSupervisedMethod,
        FineTuningJob as OpenAiFineTuningJob, FineTuningJobStatus as OpenAiFineTuningJobStatus,
    },
    models::{ListModelResponse as OpenAiListModelResponse, Model as OpenAiModel},
};
use lighthouse_protocol::{
    Capabilities, CreateFineTuningJobRequest, FineTuneJobError, FineTuneMethod,
    FineTuneSupervisedHyperparameters, FineTuneSupervisedMethod, FineTuningJob,
    FineTuningJobStatus, ListModelResponse, Model, Supports,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::fmt::Debug;

fn roundtrip<T, O>(ours: &T, extra_keys: &[&str])
where
    T: Serialize,
    O: DeserializeOwned + Serialize + Debug,
{
    let source = serde_json::to_value(ours).unwrap();
    let theirs: O = serde_json::from_value(source.clone()).unwrap();
    let roundtripped = serde_json::to_value(theirs).unwrap();
    compare_openai_keys(&source, &roundtripped, "");
    for key in extra_keys {
        assert!(
            contains_key(&source, key),
            "expected Lighthouse extra `{key}`"
        );
        assert!(
            !contains_key(&roundtripped, key),
            "OpenAI retained extra `{key}`"
        );
    }
}

fn compare_openai_keys(source: &Value, roundtripped: &Value, path: &str) {
    let (Some(source), Some(roundtripped)) = (source.as_object(), roundtripped.as_object()) else {
        assert_eq!(source, roundtripped, "OpenAI value `{path}` drifted");
        return;
    };
    for (key, value) in roundtripped {
        let path = format!("{path}/{key}");
        let original = source
            .get(key)
            .unwrap_or_else(|| panic!("OpenAI added `{path}`"));
        if value.is_object() {
            compare_openai_keys(original, value, &path);
        } else {
            assert_eq!(original, value, "OpenAI key `{path}` drifted");
        }
    }
}

fn contains_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key(key) || object.values().any(|v| contains_key(v, key))
        }
        Value::Array(array) => array.iter().any(|v| contains_key(v, key)),
        _ => false,
    }
}

fn hyperparameters() -> FineTuneSupervisedHyperparameters {
    FineTuneSupervisedHyperparameters {
        extra: serde_json::Map::from_iter([
            ("batch_size".to_owned(), json!("auto")),
            ("learning_rate_multiplier".to_owned(), json!("auto")),
            ("n_epochs".to_owned(), json!("auto")),
            ("local_epochs".to_owned(), json!(4)),
        ]),
    }
}

fn method() -> FineTuneMethod {
    FineTuneMethod {
        kind: "supervised".to_owned(),
        supervised: FineTuneSupervisedMethod {
            hyperparameters: hyperparameters(),
        },
    }
}

#[test]
fn openai_model_shapes_round_trip_and_strip_lighthouse_fields() {
    let model = Model {
        id: "m".to_owned(),
        object: "model".to_owned(),
        created: 0,
        owned_by: "provider".to_owned(),
        capabilities: Capabilities {
            classification: Some(Supports { supported: true }),
            embedding: None,
            fine_tuning: Some(Supports { supported: true }),
        },
        version: "v1".to_owned(),
        cost: 2.0,
        deterministic: true,
        abstains: false,
    };
    roundtrip::<_, OpenAiModel>(
        &model,
        &[
            "capabilities",
            "version",
            "cost",
            "deterministic",
            "abstains",
        ],
    );

    let list = ListModelResponse {
        object: "list".to_owned(),
        data: vec![model],
    };
    let source = serde_json::to_value(&list).unwrap();
    let theirs: OpenAiListModelResponse = serde_json::from_value(source).unwrap();
    let roundtripped = serde_json::to_value(theirs).unwrap();
    assert_eq!(roundtripped["data"][0]["id"], "m");
    for key in [
        "capabilities",
        "version",
        "cost",
        "deterministic",
        "abstains",
    ] {
        assert!(
            roundtripped["data"][0].get(key).is_none(),
            "OpenAI retained `{key}`"
        );
    }
}

#[test]
fn fine_tuning_openai_shapes_round_trip_and_strip_options_and_m2_fields() {
    roundtrip::<_, OpenAiFineTuneSupervisedHyperparameters>(&hyperparameters(), &["local_epochs"]);
    roundtrip::<_, OpenAiFineTuneSupervisedMethod>(
        &FineTuneSupervisedMethod {
            hyperparameters: hyperparameters(),
        },
        &[],
    );
    roundtrip::<_, OpenAiFineTuneMethod>(&method(), &[]);

    let request = CreateFineTuningJobRequest {
        model: "base".to_owned(),
        training_file: "sha256:abc".to_owned(),
        validation_file: None,
        method: method(),
        seed: 42,
        suffix: None,
        metadata: [
            ("decision".to_owned(), "d/x".to_owned()),
            ("datasets".to_owned(), "/data".to_owned()),
        ]
        .into(),
        options: serde_json::Map::from_iter([("baseUrl".to_owned(), json!("https://example"))]),
    };
    roundtrip::<_, OpenAiCreateFineTuningJobRequest>(&request, &["options"]);

    let error = FineTuneJobError {
        code: "bad_file".to_owned(),
        message: "invalid".to_owned(),
        param: Some("training_file".to_owned()),
    };
    roundtrip::<_, OpenAiFineTuneJobError>(&error, &[]);
    roundtrip::<_, OpenAiFineTuningJobStatus>(&FineTuningJobStatus::Succeeded, &[]);

    let job = FineTuningJob {
        id: "ftjob-1".to_owned(),
        created_at: 1,
        finished_at: Some(2),
        error: Some(error),
        fine_tuned_model: Some("learned@sha256:abc".to_owned()),
        hyperparameters: json!({
            "batch_size": "auto",
            "learning_rate_multiplier": "auto",
            "n_epochs": "auto"
        }),
        model: "base".to_owned(),
        object: "fine_tuning.job".to_owned(),
        organization_id: "org".to_owned(),
        result_files: vec!["file-1".to_owned()],
        status: FineTuningJobStatus::Succeeded,
        trained_tokens: Some(9),
        training_file: "file-0".to_owned(),
        validation_file: None,
        integrations: None,
        seed: 42,
        estimated_finish: None,
        method: Some(method()),
        metadata: None,
    };
    roundtrip::<_, OpenAiFineTuningJob>(&job, &[]);
}
