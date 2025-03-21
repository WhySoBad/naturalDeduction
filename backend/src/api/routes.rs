use axum::extract::{Path, State};
use axum::Json;
use log::info;
use sea_orm::{ActiveModelTrait, IntoActiveModel, ModelTrait, QueryFilter, TransactionTrait};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use crate::db::*;
use crate::error::{BackendError, BackendResult};
use crate::lib::derivation::formula::Formula;
use crate::lib::derivation::statement::Statement;
use crate::lib::derivation::tree::{check_tree, infer_mapping_stmt};
use crate::lib::rule::{DerivationRule, RuleIdentifier, Rules};
use crate::AppState;
use sea_orm::EntityTrait;

use super::models::{
    ApplyRuleParams, CreateExerciseRequest, CreateTreeRequest, ElementMapping, Exercise, Feedback,
    FormulaMapping, Node, ParseParams, SideCondition, Tipp,
};
use crate::lib::{db, LogicParser};
use sea_orm::ColumnTrait;

#[utoipa::path(
    get,
    path = "/api/exercise",
    responses(
        (status = StatusCode::OK, body = Vec<Exercise>),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
pub async fn get_exercises(state: State<AppState>) -> BackendResult<Json<Vec<Exercise>>> {
    let exercises = exercise::Entity::find().all(&state.db).await?;

    let mut result = Vec::new();
    for e in exercises.iter() {
        let exercise = statement::Entity::find_by_id(e.statement_id)
            .one(&state.db)
            .await?;
        if exercise.is_none() {
            continue;
        }
        let exercise = exercise.unwrap();
        let hint_available = node::Entity::find()
            .filter(node::Column::ParentId.eq(exercise.id))
            .one(&state.db)
            .await?
            .is_some();
        let formula = serde_json::from_str::<Formula>(&exercise.rhs)
            .map_err(|e| BackendError::BadRequest(format!("failed to serialize: {e}")))?;

        let lhs = serde_json::from_str::<Vec<Formula>>(&exercise.lhs)
            .map_err(|e| BackendError::BadRequest(format!("failed to serialize: {e}")))?;

        let sidecondition = serde_json::from_str::<Vec<_>>(&exercise.sidecondition)
            .map_err(|e| BackendError::BadRequest(format!("failed to serialize asdf: {e}")))?;

        result.push(Exercise {
            id: e.id,
            likes: e.likes,
            dislikes: e.dislikes,
            difficulty: e.difficulty,
            exercise: Statement {
                lhs,
                formula,
                sidecondition,
            },
            hint: hint_available,
        });
    }

    Ok(Json(result))
}

#[utoipa::path(
    get,
    path = "/api/exercise/{id}",
    responses(
        (status = StatusCode::OK, body = Statement),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
pub async fn get_exercise(
    state: State<AppState>,
    Path(id): Path<Uuid>,
) -> BackendResult<Json<Statement>> {
    let exercises = exercise::Entity::find_by_id(id).one(&state.db).await?;

    if exercises.is_none() {
        return Err(BackendError::NotFound {
            entity: "Exercise".to_string(),
        });
    }

    let statement = statement::Entity::find_by_id(exercises.unwrap().statement_id)
        .one(&state.db)
        .await?;

    if statement.is_none() {
        return Err(BackendError::NotFound {
            entity: "Statement".to_string(),
        });
    }

    let statement = statement.unwrap();
    let formula = serde_json::from_str::<Formula>(&statement.rhs)
        .map_err(|e| BackendError::BadRequest(format!("failed to serialize: {e}")))?;

    let lhs = serde_json::from_str::<Vec<Formula>>(&statement.lhs)
        .map_err(|e| BackendError::BadRequest(format!("failed to serialize: {e}")))?;

    let sidecondition = serde_json::from_str::<Vec<SideCondition>>(&statement.sidecondition)
        .map_err(|e| BackendError::BadRequest(format!("failed to serialize: {e}")))?;

    let exercise = Statement {
        formula,
        lhs,
        sidecondition,
    };

    Ok(Json(exercise))
}

#[utoipa::path(
    post,
    path = "/api/exercise",
    responses(
        (status = StatusCode::OK, body = bool),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
pub async fn create_exercise(
    state: State<AppState>,
    query: Json<CreateExerciseRequest>,
) -> BackendResult<Json<bool>> {
    let res = query.statement.check();

    if !res {
        return Err(BackendError::BadRequest(
            "The formula is not a tautology".to_string(),
        ));
    }

    let rhs = serde_json::to_string(&query.statement.formula)
        .map_err(|e| BackendError::BadRequest(format!("failed to serialize: {e}")))?;
    let lhs = serde_json::to_string(&query.statement.lhs)
        .map_err(|e| BackendError::BadRequest(format!("failed to serialize: {e}")))?;

    println!("{:?}", query.statement.sidecondition);
    let sidecondition = serde_json::to_string(&query.statement.sidecondition)
        .map_err(|e| BackendError::BadRequest(format!("failed to serialize: {e}")))?;

    let exists = statement::Entity::find()
        .filter(statement::Column::Lhs.eq(&lhs))
        .filter(statement::Column::Rhs.eq(&rhs))
        .filter(statement::Column::Sidecondition.eq(&sidecondition))
        .one(&state.db)
        .await?;

    let exercise = if let Some(stmt) = exists {
        let ex = exercise::Entity::find()
            .filter(exercise::Column::StatementId.eq(stmt.id))
            .one(&state.db)
            .await?;
        match ex {
            Some(_) => {
                return Err(BackendError::BadRequest(
                    "This exercise already exists".to_string(),
                ))
            }
            None => exercise::ActiveModel {
                dislikes: sea_orm::ActiveValue::Set(0),
                likes: sea_orm::ActiveValue::Set(0),
                statement_id: sea_orm::ActiveValue::Set(stmt.id),
                ..Default::default()
            },
        }
    } else {
        let node = statement::ActiveModel {
            lhs: sea_orm::ActiveValue::Set(lhs),
            rhs: sea_orm::ActiveValue::Set(rhs),
            sidecondition: sea_orm::ActiveValue::Set(sidecondition),
            ..Default::default()
        };

        let statement = node.save(&state.db).await?;

        exercise::ActiveModel {
            dislikes: sea_orm::ActiveValue::Set(0),
            likes: sea_orm::ActiveValue::Set(0),
            statement_id: statement.id,
            ..Default::default()
        }
    };
    let _ = exercise.save(&state.db).await?;

    Ok(Json(true))
}

#[utoipa::path(
    post,
    path = "/api/parse",
    responses(
        (status = StatusCode::OK, body = Formula),
        (status = StatusCode::NOT_FOUND, description = "Building not found"),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
pub async fn parse(query: Json<ParseParams>) -> BackendResult<Json<Formula>> {
    let f = LogicParser::parse_input(&query.formula);
    match f {
        Ok(formula) => Ok(Json(formula)),
        Err(err) => Err(BackendError::BadRequest(err)),
    }
}

#[utoipa::path(
    get,
    path = "/api/rules",
    responses(
        (status = StatusCode::OK, body = Vec<DerivationRule>),
        (status = StatusCode::NOT_FOUND, description = "Building not found"),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
pub async fn all_rules() -> BackendResult<Json<Vec<DerivationRule>>> {
    let rules = Rules::all_rules();
    Ok(Json(rules.into_iter().collect::<Vec<_>>()))
}

#[utoipa::path(
    post,
    path = "/api/apply",
    responses(
        (status = StatusCode::OK, body = Vec<Statement>),
        (status = StatusCode::NOT_FOUND, description = "Building not found"),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
pub async fn apply_rule(query: Json<ApplyRuleParams>) -> BackendResult<Json<Vec<Statement>>> {
    let rule = query.rule.get_rule();

    let mut formula_mapping = query
        .mapping
        .clone()
        .into_iter()
        .map(|map| (RuleIdentifier::Formula(map.from), map.to))
        .collect::<BTreeMap<RuleIdentifier, Formula>>();

    let mut element_mapping = query
        .substitution
        .clone()
        .into_iter()
        .map(|map| (RuleIdentifier::Element(map.from), map.to))
        .collect::<BTreeMap<RuleIdentifier, String>>();

    let new_premisses =
        query
            .statement
            .apply_rule(rule, &mut formula_mapping, &mut element_mapping)?;
    Ok(Json(new_premisses))
}

#[utoipa::path(
    post,
    path = "/api/check",
    responses(
        (status = StatusCode::OK, body = bool),
        (status = StatusCode::NOT_FOUND, description = "Building not found"),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
pub async fn check(query: Json<CreateExerciseRequest>) -> BackendResult<Json<bool>> {
    let result = query.statement.check();
    info!("{:?} is a tautology: {}", query.0, result);
    Ok(Json(result))
}

#[utoipa::path(
    post,
    path = "/api/add_tree",
    responses(
        (status = StatusCode::OK, body = bool),
        (status = StatusCode::NOT_FOUND, description = "Building not found"),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
pub async fn add_tree(
    state: State<AppState>,
    query: Json<CreateTreeRequest>,
) -> BackendResult<Json<bool>> {
    check_tree(query.root_id, &query.nodes)?;
    let trx = state.db.begin().await?;
    let _ = db::add_tree(&trx, query.root_id, &query.nodes).await?;
    trx.commit().await?;
    Ok(Json(true))
}

#[utoipa::path(
    post,
    path = "/api/exercise/{id}/feedback",
    responses(
        (status = StatusCode::OK, body = bool),
        (status = StatusCode::NOT_FOUND, description = "Building not found"),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
pub async fn post_feedback(
    state: State<AppState>,
    Path(id): Path<Uuid>,
    query: Json<Feedback>,
) -> BackendResult<Json<bool>> {
    let exercise = exercise::Entity::find_by_id(id).one(&state.db).await?;

    if exercise.is_none() {
        return Err(BackendError::NotFound {
            entity: "Exercise".to_string(),
        });
    }

    let exercise = exercise.unwrap();
    let mut active_model = exercise.clone().into_active_model();

    if query.like {
        active_model.likes = sea_orm::ActiveValue::Set(exercise.likes + 1);
    } else {
        active_model.dislikes = sea_orm::ActiveValue::Set(exercise.dislikes + 1);
    }

    if let Some(ranking) = query.difficulty {
        let before_average = exercise.difficulty * exercise.num_responses as f64;
        let new_average = (before_average + ranking as f64) / (exercise.num_responses + 1) as f64;
        active_model.difficulty = sea_orm::ActiveValue::Set(new_average);
        active_model.num_responses = sea_orm::ActiveValue::Set(exercise.num_responses + 1);
    }

    let _ = active_model.save(&state.db).await?;

    Ok(Json(true))
}

#[utoipa::path(
    post,
    path = "/api/statement/hint",
    responses(
        (status = StatusCode::OK, body = Vec<Tipp>),
        (status = StatusCode::INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
pub async fn get_tipp(
    state: State<AppState>,
    query: Json<Statement>,
) -> BackendResult<Json<Vec<Tipp>>> {
    let lhs = serde_json::to_string(&query.lhs)
        .map_err(|e| BackendError::BadRequest(format!("failed to serialize: {e}")))?;
    let rhs = serde_json::to_string(&query.formula)
        .map_err(|e| BackendError::BadRequest(format!("failed to serialize: {e}")))?;

    let statement = statement::Entity::find()
        .filter(statement::Column::Lhs.eq(&lhs))
        .filter(statement::Column::Rhs.eq(&rhs))
        .one(&state.db)
        .await?;

    if statement.is_none() {
        return Err(BackendError::NotFound {
            entity: "Statement".to_string(),
        });
    }
    let statement = statement.unwrap();

    let tipps = node::Entity::find()
        .filter(node::Column::ParentId.eq(statement.id))
        .all(&state.db)
        .await?;
    let mut sorted_tips: BTreeMap<Rules, Vec<(Statement, u32)>> = BTreeMap::new();

    for node in tipps {
        let mut premisses = sorted_tips
            .get(&node.rule.clone().into())
            .unwrap_or(&Vec::new())
            .clone();

        if let Some(premisse) = node.child_id {
            let premiss = statement::Entity::find_by_id(premisse)
                .one(&state.db)
                .await?;

            if let Some(premisse) = premiss {
                premisses.push((
                    Statement {
                        lhs: serde_json::from_str(&premisse.lhs).unwrap(),
                        formula: serde_json::from_str(&premisse.rhs).unwrap(),
                        sidecondition: serde_json::from_str(&premisse.sidecondition).unwrap(),
                    },
                    node.order as u32,
                ));
            }
        }
        sorted_tips.insert(node.rule.into(), premisses);
    }

    let result = sorted_tips
        .iter()
        .map(|(rule, premisses)| {
            let mut sorted_premisses = premisses.clone();
            sorted_premisses.sort_by(|(_, a), (_, b)| a.cmp(b));
            Tipp {
                rule: rule.clone(),
                premisses: sorted_premisses.iter().map(|(s, _)| s.clone()).collect(),
            }
        })
        .collect::<Vec<_>>();
    Ok(Json(result))
}
