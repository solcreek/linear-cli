//! Every GraphQL document linearctl sends. The tests validate each against
//! graphql/schema.graphql, so a field Linear renames fails the tests rather
//! than a user. Update the schema with `make schema`.

/// What a list shows for an issue. Search results are a different type with
/// the same fields, so it is a field list, not a fragment. Nested objects
/// carry their id, so a caller can join them with state, label, user, team,
/// project and cycle list: names are not unique across teams.
macro_rules! issue_row {
    () => {
        "\n\tid identifier title priority priorityLabel url createdAt updatedAt\n\tstate { id name type color }\n\tassignee { id name email }\n\tdelegate { id name }\n\tteam { id key }\n\tproject { id name }\n\tcycle { id number }\n\tprojectMilestone { id name }\n\tlabels(first: 20) { nodes { id name color } }"
    };
}

macro_rules! page_info {
    () => {
        "pageInfo { hasNextPage endCursor }"
    };
}

macro_rules! comment_row {
    () => {
        "id body createdAt url user { name email } parent { id }"
    };
}

/// The keys of an issue_row item, for --fields.
pub const ISSUE_ROW_FIELDS: &[&str] = &[
    "id",
    "identifier",
    "title",
    "priority",
    "priorityLabel",
    "url",
    "createdAt",
    "updatedAt",
    "state",
    "assignee",
    "delegate",
    "team",
    "project",
    "cycle",
    "projectMilestone",
    "labels",
];

pub const VIEWER: &str = "query Viewer { viewer { id name email } organization { name urlKey } }";

pub const ISSUE_LIST: &str = concat!(
    "query IssueList($filter: IssueFilter, $first: Int!, $after: String, $orderBy: PaginationOrderBy, $sort: [IssueSortInput!]) {\n  issues(filter: $filter, first: $first, after: $after, orderBy: $orderBy, sort: $sort) { nodes {",
    issue_row!(),
    " } ",
    page_info!(),
    " }\n}"
);

pub const ISSUE_SEARCH: &str = concat!(
    "query IssueSearch($term: String!, $filter: IssueFilter, $first: Int!, $after: String, $orderBy: PaginationOrderBy,\n  $includeComments: Boolean) {\n  searchIssues(term: $term, filter: $filter, first: $first, after: $after, orderBy: $orderBy, includeComments: $includeComments) { nodes {",
    issue_row!(),
    " } ",
    page_info!(),
    " }\n}"
);

pub const ISSUE_SHOW: &str = concat!(
    "query IssueShow($id: String!, $comments: Int!, $withComments: Boolean!) {\n  issue(id: $id) {",
    issue_row!(),
    "\n    description dueDate estimate branchName startedAt completedAt canceledAt archivedAt\n    creator { name email }\n    parent { identifier title }\n    children(first: 50) { nodes { identifier title state { name type } } }\n    relations(first: 50) { nodes { id type relatedIssue { identifier title } } }\n    inverseRelations(first: 50) { nodes { id type issue { identifier title } } }\n    attachments(first: 50) { nodes { title url } }\n    comments(first: $comments) @include(if: $withComments) { nodes { id body createdAt url user { name email } parent { id } } }\n  }\n}"
);

/// What issue create needs resolved, in one request: names to ids, and
/// whether an issue with the chosen id already exists.
pub const CREATE_RESOLVE: &str = concat!(
    "query CreateResolve($team: String!, $id: ID!, $withID: Boolean!, $state: String!, $withState: Boolean!,\n  $email: String!, $withUser: Boolean!, $me: Boolean!, $labels: IssueLabelFilter!, $withLabels: Boolean!,\n  $project: String!, $withProject: Boolean!, $parent: IssueFilter!, $withParent: Boolean!,\n  $milestone: String!, $withMilestone: Boolean!, $delegate: String!, $withDelegate: Boolean!,\n  $stateType: String!, $withStateType: Boolean!) {\n  teams(filter: { key: { eqIgnoreCase: $team } }) { nodes { id key\n    states(filter: { name: { eqIgnoreCase: $state } }) @include(if: $withState) { nodes { id name } }\n    typed: states(filter: { type: { eq: $stateType } }) @include(if: $withStateType) { nodes { id name position } } } }\n  existing: issues(filter: { id: { eq: $id } }, includeArchived: true) @include(if: $withID) { nodes {",
    issue_row!(),
    " } }\n  users(filter: { email: { eqIgnoreCase: $email } }) @include(if: $withUser) { nodes { id name email } }\n  viewer @include(if: $me) { id name email app }\n  issueLabels(filter: $labels) @include(if: $withLabels) { nodes { id name team { id } } }\n  projects(filter: { name: { eqIgnoreCase: $project } }) @include(if: $withProject) { nodes { id name } }\n  parent: issues(filter: $parent, first: 1) @include(if: $withParent) { nodes { id identifier } }\n  milestones: projectMilestones(filter: { name: { eqIgnoreCase: $milestone } }) @include(if: $withMilestone) { nodes { id name project { id name } } }\n  delegates: users(filter: { app: { eq: true }, or: [{ name: { eqIgnoreCase: $delegate } }, { displayName: { eqIgnoreCase: $delegate } }, { email: { eqIgnoreCase: $delegate } }] }) @include(if: $withDelegate) { nodes { id name } }\n}"
);

pub const ISSUE_CREATE: &str = concat!(
    "mutation IssueCreate($input: IssueCreateInput!) {\n  issueCreate(input: $input) { success issue {",
    issue_row!(),
    " } }\n}"
);

/// The issue as it is now, and what issue update needs resolved.
pub const UPDATE_RESOLVE: &str = "query UpdateResolve($id: String!, $state: String!, $withState: Boolean!,\n  $email: String!, $withUser: Boolean!, $me: Boolean!, $labels: IssueLabelFilter!, $withLabels: Boolean!,\n  $project: String!, $withProject: Boolean!, $parent: IssueFilter!, $withParent: Boolean!,\n  $milestone: String!, $withMilestone: Boolean!, $delegate: String!, $withDelegate: Boolean!,\n  $stateType: String!, $withStateType: Boolean!) {\n  issue(id: $id) { id identifier title description priority dueDate estimate updatedAt\n    team { id key states(filter: { name: { eqIgnoreCase: $state } }) @include(if: $withState) { nodes { id name } }\n    typed: states(filter: { type: { eq: $stateType } }) @include(if: $withStateType) { nodes { id name position } } }\n    state { id name } assignee { id email } project { id name } parent { id identifier }\n    projectMilestone { id } delegate { id }\n    labels(first: 100) { nodes { id name } } }\n  users(filter: { email: { eqIgnoreCase: $email } }) @include(if: $withUser) { nodes { id name email } }\n  viewer @include(if: $me) { id name email app }\n  issueLabels(filter: $labels) @include(if: $withLabels) { nodes { id name team { id } } }\n  projects(filter: { name: { eqIgnoreCase: $project } }) @include(if: $withProject) { nodes { id name } }\n  parent: issues(filter: $parent, first: 1) @include(if: $withParent) { nodes { id identifier } }\n  milestones: projectMilestones(filter: { name: { eqIgnoreCase: $milestone } }) @include(if: $withMilestone) { nodes { id name project { id name } } }\n  delegates: users(filter: { app: { eq: true }, or: [{ name: { eqIgnoreCase: $delegate } }, { displayName: { eqIgnoreCase: $delegate } }, { email: { eqIgnoreCase: $delegate } }] }) @include(if: $withDelegate) { nodes { id name } }\n}";

pub const ISSUE_UPDATE: &str = concat!(
    "mutation IssueUpdate($id: String!, $input: IssueUpdateInput!) {\n  issueUpdate(id: $id, input: $input) { success issue {",
    issue_row!(),
    " } }\n}"
);

pub const ISSUE_ARCHIVED: &str =
    "query IssueArchived($id: String!) { issue(id: $id) { id identifier title url archivedAt } }";

pub const ISSUE_ARCHIVE: &str = "mutation IssueArchive($id: String!) { issueArchive(id: $id) { success } }";

pub const ISSUE_UNARCHIVE: &str = "mutation IssueUnarchive($id: String!) { issueUnarchive(id: $id) { success } }";

pub const COMMENT_LIST: &str = concat!(
    "query CommentList($id: String!, $first: Int!, $after: String) {\n  issue(id: $id) { comments(first: $first, after: $after) { nodes { ",
    comment_row!(),
    " } ",
    page_info!(),
    " } }\n}"
);

pub const COMMENT_RESOLVE: &str = concat!(
    "query CommentResolve($issue: String!, $id: ID!) {\n  issue(id: $issue) { id identifier }\n  comments(filter: { id: { eq: $id } }) { nodes { ",
    comment_row!(),
    " } }\n}"
);

pub const COMMENT_CREATE: &str = concat!(
    "mutation CommentCreate($input: CommentCreateInput!) {\n  commentCreate(input: $input) { success comment { ",
    comment_row!(),
    " } }\n}"
);

pub const COMMENT: &str = concat!("query Comment($id: String!) { comment(id: $id) { ", comment_row!(), " } }");

pub const COMMENT_UPDATE: &str = concat!(
    "mutation CommentUpdate($id: String!, $input: CommentUpdateInput!) {\n  commentUpdate(id: $id, input: $input) { success comment { ",
    comment_row!(),
    " } }\n}"
);

pub const COMMENT_DELETE: &str = "mutation CommentDelete($id: String!) { commentDelete(id: $id) { success } }";

/// Both issues by filter (a missing one is an empty list, not an error),
/// with the relations stored from and to the first.
pub const RELATION_RESOLVE: &str = "query RelationResolve($a: IssueFilter!, $b: IssueFilter!) {\n  a: issues(filter: $a, first: 1, includeArchived: true) { nodes { id identifier\n    relations(first: 250) { nodes { id type relatedIssue { id identifier } } }\n    inverseRelations(first: 250) { nodes { id type issue { id identifier } } } } }\n  b: issues(filter: $b, first: 1, includeArchived: true) { nodes { id identifier } }\n}";

pub const RELATION_CREATE: &str = "mutation RelationCreate($input: IssueRelationCreateInput!) {\n  issueRelationCreate(input: $input) { success issueRelation { id } }\n}";

pub const RELATION_DELETE: &str = "mutation RelationDelete($id: String!) { issueRelationDelete(id: $id) { success } }";

/// watch: the ids that match now; why ids stopped matching; what changed
/// since a time, archived included; and the issue changes to subscribe to.
pub const WATCH_IDS: &str = concat!(
    "query WatchIDs($filter: IssueFilter, $first: Int!, $after: String) {\n  issues(filter: $filter, first: $first, after: $after) { nodes { id identifier } ",
    page_info!(),
    " }\n}"
);

pub const WATCH_GONE: &str = "query WatchGone($ids: [ID!]!) {\n  issues(filter: { id: { in: $ids } }, first: 250, includeArchived: true) { nodes { id identifier archivedAt trashed } }\n}";

pub const WATCH_CHANGED: &str = "query WatchChanged($filter: IssueFilter!) {\n  issues(filter: $filter, first: 250, includeArchived: true, orderBy: updatedAt) { nodes { id } }\n}";

pub const WATCH_CREATED: &str = "subscription WatchCreated { issueCreated { id } }";
pub const WATCH_UPDATED: &str = "subscription WatchUpdated { issueUpdated { id } }";
pub const WATCH_ARCHIVED: &str = "subscription WatchArchived { issueArchived { id } }";
pub const WATCH_UNARCHIVED: &str = "subscription WatchUnarchived { issueUnarchived { id } }";

pub const TEAMS: &str = concat!(
    "query Teams($first: Int!, $after: String) {\n  teams(first: $first, after: $after) { nodes { id key name description private } ",
    page_info!(),
    " }\n}"
);

pub const STATES: &str = concat!(
    "query States($filter: WorkflowStateFilter, $first: Int!, $after: String) {\n  workflowStates(filter: $filter, first: $first, after: $after) { nodes { id name type position color team { key } } ",
    page_info!(),
    " }\n}"
);

pub const LABELS: &str = concat!(
    "query Labels($filter: IssueLabelFilter, $first: Int!, $after: String) {\n  issueLabels(filter: $filter, first: $first, after: $after) { nodes { id name color isGroup team { key } parent { name } } ",
    page_info!(),
    " }\n}"
);

pub const USERS: &str = concat!(
    "query Users($filter: UserFilter, $first: Int!, $after: String) {\n  users(filter: $filter, first: $first, after: $after) { nodes { id name displayName email active admin guest app } ",
    page_info!(),
    " }\n}"
);

pub const PROJECTS: &str = concat!(
    "query Projects($filter: ProjectFilter, $first: Int!, $after: String) {\n  projects(filter: $filter, first: $first, after: $after) { nodes { id name url targetDate status { name type } lead { name email } } ",
    page_info!(),
    " }\n}"
);

/// first: 2 so a name two projects share shows up as ambiguous.
pub const PROJECT_FIND: &str = concat!(
    "query ProjectFind($filter: ProjectFilter!, $issues: Int!, $withIssues: Boolean!) {\n  projects(filter: $filter, first: 2) { nodes {\n    id name slugId url description content startDate targetDate progress\n    status { id name type } lead { id name email }\n    teams(first: 20) { nodes { id key } }\n    projectMilestones(first: 50) { nodes { id name targetDate progress } }\n    issues(first: $issues) @include(if: $withIssues) { nodes {",
    issue_row!(),
    " } }\n  } }\n}"
);

pub const PROJECT_UPDATE: &str = "mutation ProjectUpdate($id: String!, $input: ProjectUpdateInput!) {\n  projectUpdate(id: $id, input: $input) { success }\n}";

pub const MILESTONES: &str = concat!(
    "query Milestones($filter: ProjectMilestoneFilter, $first: Int!, $after: String) {\n  projectMilestones(filter: $filter, first: $first, after: $after) { nodes { id name targetDate progress project { id name } } ",
    page_info!(),
    " }\n}"
);

pub const NOTIFICATIONS: &str = concat!(
    "query Notifications($filter: NotificationFilter, $first: Int!, $after: String) {\n  notifications(filter: $filter, first: $first, after: $after) { nodes {\n    id type title subtitle url createdAt readAt snoozedUntilAt actor { id name }\n    ... on IssueNotification { issue { id identifier title } comment { id } }\n  } ",
    page_info!(),
    " }\n}"
);

/// A cycle's own fields under Linear's names: what cycle list shows, and
/// what cycle show starts from. progress is Linear's (completed points plus a
/// quarter of in-progress ones, over all points); nothing here is computed.
macro_rules! cycle_row {
    () => {
        "id number name description startsAt endsAt completedAt isActive isNext isPrevious isPast isFuture progress team { id key }"
    };
}

pub const CYCLES: &str = concat!(
    "query Cycles($filter: CycleFilter, $first: Int!, $after: String) {\n  cycles(filter: $filter, first: $first, after: $after) { nodes { ",
    cycle_row!(),
    " } ",
    page_info!(),
    " }\n}"
);

/// The cycle a change applies to, as it is now; two so an ambiguous CYCLE
/// shows up.
pub const CYCLE_FIND: &str = concat!(
    "query CycleFind($filter: CycleFilter!) {\n  cycles(filter: $filter, first: 2, includeArchived: true) { nodes { ",
    cycle_row!(),
    " archivedAt } }\n}"
);

/// cycle create's team, and whether a cycle with the chosen id exists.
pub const CYCLE_CREATE_RESOLVE: &str = concat!(
    "query CycleCreateResolve($team: String!, $id: ID!) {\n  teams(filter: { key: { eqIgnoreCase: $team } }) { nodes { id key } }\n  existing: cycles(filter: { id: { eq: $id } }, includeArchived: true) { nodes { ",
    cycle_row!(),
    " } }\n}"
);

pub const CYCLE_CREATE: &str = concat!(
    "mutation CycleCreate($input: CycleCreateInput!) {\n  cycleCreate(input: $input) { success cycle { ",
    cycle_row!(),
    " } }\n}"
);

pub const CYCLE_UPDATE: &str = concat!(
    "mutation CycleUpdate($id: String!, $input: CycleUpdateInput!) {\n  cycleUpdate(id: $id, input: $input) { success cycle { ",
    cycle_row!(),
    " } }\n}"
);

pub const CYCLE_ARCHIVE: &str = "mutation CycleArchive($id: String!) { cycleArchive(id: $id) { success } }";

/// matches asks for two, so a CYCLE that names a cycle in two teams shows up
/// as ambiguous; the detail asks for one, since first multiplies the cost of
/// everything nested (two at 250 issues each was 14,027 points of 10,000).
/// Archived cycles count: a UUID may name one.
pub const CYCLE_SHOW: &str = concat!(
    "query CycleShow($filter: CycleFilter!, $issues: Int!, $withIssues: Boolean!, $after: String,\n  $uncompleted: Int!, $withUncompleted: Boolean!) {\n  matches: cycles(filter: $filter, first: 2, includeArchived: true) { nodes { id team { key } } }\n  cycles(filter: $filter, first: 1, includeArchived: true) { nodes {\n    ",
    cycle_row!(),
    "\n    createdAt updatedAt archivedAt autoArchivedAt\n    currentProgress progressHistory scopeHistory completedScopeHistory inProgressScopeHistory\n    issueCountHistory completedIssueCountHistory\n    inheritedFrom { id number name team { id key } }\n    issues(first: $issues, after: $after) @include(if: $withIssues) { nodes {",
    issue_row!(),
    " } ",
    page_info!(),
    " }\n    uncompletedIssuesUponClose(first: $uncompleted) @include(if: $withUncompleted) { nodes {",
    issue_row!(),
    " } ",
    page_info!(),
    " }\n  } }\n}"
);

/// Every document above, by name, for the schema test.
#[cfg(test)]
pub fn documents() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Viewer", VIEWER),
        ("IssueList", ISSUE_LIST),
        ("IssueSearch", ISSUE_SEARCH),
        ("IssueShow", ISSUE_SHOW),
        ("CreateResolve", CREATE_RESOLVE),
        ("IssueCreate", ISSUE_CREATE),
        ("UpdateResolve", UPDATE_RESOLVE),
        ("IssueUpdate", ISSUE_UPDATE),
        ("IssueArchived", ISSUE_ARCHIVED),
        ("IssueArchive", ISSUE_ARCHIVE),
        ("IssueUnarchive", ISSUE_UNARCHIVE),
        ("CommentList", COMMENT_LIST),
        ("CommentResolve", COMMENT_RESOLVE),
        ("CommentCreate", COMMENT_CREATE),
        ("RelationResolve", RELATION_RESOLVE),
        ("RelationCreate", RELATION_CREATE),
        ("RelationDelete", RELATION_DELETE),
        ("Comment", COMMENT),
        ("CommentUpdate", COMMENT_UPDATE),
        ("CommentDelete", COMMENT_DELETE),
        ("Teams", TEAMS),
        ("States", STATES),
        ("Labels", LABELS),
        ("Users", USERS),
        ("Projects", PROJECTS),
        ("Cycles", CYCLES),
        ("CycleShow", CYCLE_SHOW),
        ("CycleFind", CYCLE_FIND),
        ("CycleCreateResolve", CYCLE_CREATE_RESOLVE),
        ("CycleCreate", CYCLE_CREATE),
        ("CycleUpdate", CYCLE_UPDATE),
        ("CycleArchive", CYCLE_ARCHIVE),
        ("WatchIDs", WATCH_IDS),
        ("WatchGone", WATCH_GONE),
        ("WatchChanged", WATCH_CHANGED),
        ("WatchCreated", WATCH_CREATED),
        ("WatchUpdated", WATCH_UPDATED),
        ("WatchArchived", WATCH_ARCHIVED),
        ("WatchUnarchived", WATCH_UNARCHIVED),
        ("Notifications", NOTIFICATIONS),
        ("Milestones", MILESTONES),
        ("ProjectFind", PROJECT_FIND),
        ("ProjectUpdate", PROJECT_UPDATE),
    ]
}

#[cfg(test)]
mod tests {
    use apollo_compiler::{ExecutableDocument, Schema};

    /// Every document linearctl sends, validated against Linear's published
    /// schema (graphql/schema.graphql).
    #[test]
    fn documents_match_the_schema() {
        let sdl = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/graphql/schema.graphql")).unwrap();
        let schema = Schema::parse_and_validate(sdl, "schema.graphql").expect("Linear's schema");
        for (name, doc) in super::documents() {
            let parsed = ExecutableDocument::parse_and_validate(&schema, doc, format!("{name}.graphql"));
            if let Err(e) = parsed {
                panic!("{name}: {}", e.errors);
            }
            let op = doc.split_whitespace().nth(1).unwrap().split('(').next().unwrap();
            assert_eq!(op, name, "document {name} is named {op}");
        }
        // And the check can fail.
        assert!(ExecutableDocument::parse_and_validate(&schema, "query X { viewer { nope } }", "x.graphql").is_err());
    }
}
