use super::{EMPTY_OBJECT_SCHEMA, strict, xml_escape_catalog};
use crate::json::{Shape, Ty, field};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

pub struct ListSkillsTool;

#[async_trait::async_trait]
impl Tool for ListSkillsTool {
    fn name(&self) -> &'static str {
        names::LIST_SKILLS
    }

    fn description(&self) -> &'static str {
        "List available Agent Skills as an <available_skills> catalog (name + description). Call \
this when a task may match a specialized skill, then activate_skill with the skill's \
name to load full instructions."
    }

    fn schema(&self) -> String {
        EMPTY_OBJECT_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let list = h.list_skills(&c.workspace_id, &c.agent_code).await?;
        // Agent Skills client shape (agentskills.io): name + description.
        // location is omitted; activate_skill loads the body.
        let mut b = String::from("<available_skills>\n");
        for s in &list {
            b.push_str("  <skill>\n");
            b.push_str(&format!(
                "    <name>{}</name>\n",
                xml_escape_catalog(&s.name)
            ));
            if !s.description.is_empty() {
                b.push_str(&format!(
                    "    <description>{}</description>\n",
                    xml_escape_catalog(&s.description)
                ));
            }
            b.push_str("  </skill>\n");
        }
        b.push_str("</available_skills>");
        Ok(b)
    }
}

const ACTIVATE_SKILL_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "name": {"type": "string", "description": "Skill name from list_skills (e.g. go-code-review)"}
  },
  "required": ["name"]
}"#;

static ACTIVATE_SKILL_IN: Shape = Shape::anon(&[field("name", Ty::Str)]);

pub struct ActivateSkillTool;

#[async_trait::async_trait]
impl Tool for ActivateSkillTool {
    fn name(&self) -> &'static str {
        names::ACTIVATE_SKILL
    }

    fn description(&self) -> &'static str {
        "Load the full instructions for one Agent Skill by name (from list_skills). Returns the \
skill body wrapped for use in this turn. Prefer this over reading SKILL.md paths \
yourself. Do not re-activate a skill already present as <skill_content> in the user \
message."
    }

    fn schema(&self) -> String {
        ACTIVATE_SKILL_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &ACTIVATE_SKILL_IN)?;
        let name = a.str("name").trim();
        if name.is_empty() {
            return Err("activate_skill: name is required".into());
        }
        Ok(h.activate_skill(&c.workspace_id, &c.agent_code, name)
            .await?)
    }
}
