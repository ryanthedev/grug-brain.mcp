use crate::domain::ports::ConversationPort;
use crate::tools::conversation::grug_conversation;
use crate::tools::GrugDb;

impl ConversationPort for GrugDb {
    fn grug_conversation(
        &mut self,
        action: &str,
        title: Option<&str>,
        message: Option<&str>,
        identity: Option<&str>,
        status: Option<&str>,
        brain: Option<&str>,
    ) -> Result<String, String> {
        grug_conversation(self, action, title, message, identity, status, brain)
    }
}
