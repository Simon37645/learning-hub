//! 领域模型。全部是纯数据 + 少量纯函数，不依赖 Tauri，方便单测与复用。

pub mod card;
pub mod note;
pub mod quiz;
pub mod session;
pub mod stage;
pub mod task;
pub mod topic;

pub use card::{Card, Grade, SrsState};
pub use note::{Note, NoteSummary};
pub use quiz::{Answer, Attempt, Question, QuestionType, Quiz};
pub use session::StudySession;
pub use stage::StudyStage;
pub use task::{PlanTask, TaskStatus};
pub use topic::{Topic, TopicMeta, TopicStats, TopicSummary, Workspace};
