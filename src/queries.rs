//! Every GraphQL document linearctl sends. The tests validate each against
//! graphql/schema.graphql, so a field Linear renames fails the tests rather
//! than a user. Update the schema with `make schema`.

/// What a list shows for an issue. Search results are a different type with
/// the same fields, so it is a field list, not a fragment. Nested objects
/// carry their id, so a caller can join them with state, label, user, team,
/// project and cycle list: names are not unique across teams.
macro_rules! issue_row {
    () => {
        "\n\tid identifier title priority priorityLabel url createdAt updatedAt dueDate estimate\n\tstate { id name type color }\n\tassignee { id name email }\n\tdelegate { id name }\n\tteam { id key name }\n\tproject { id name }\n\tcycle { id number }\n\tprojectMilestone { id name }\n\tlabels(first: 20) { nodes { id name color } }\n\tlabelIds"
    };
}

/// issue show's collections, read in full: 250 at a time, the first page
/// in IssueShow (""), the rest in IssueShowMore ("more"), whose cursors
/// are its variables.
macro_rules! show_connections {
    ("") => {
        concat!(
            "allLabels: labels(first: 250) { nodes { id name color } ", page_info!(), " }\n    ",
            "children(first: 250) { nodes { identifier title state { name type } } ", page_info!(), " }\n    ",
            "relations(first: 250) { nodes { ", relation_node!(), " } ", page_info!(), " }\n    ",
            "inverseRelations(first: 250) { nodes { ", relation_node!(), " } ", page_info!(), " }\n    ",
            "attachments(first: 250) { nodes { title url } ", page_info!(), " }"
        )
    };
    ("more") => {
        concat!(
            "allLabels: labels(first: 250, after: $labels) @include(if: $withLabels) { nodes { id name color } ", page_info!(), " }\n    ",
            "children(first: 250, after: $children) @include(if: $withChildren) { nodes { identifier title state { name type } } ", page_info!(), " }\n    ",
            "relations(first: 250, after: $relations) @include(if: $withRelations) { nodes { ", relation_node!(), " } ", page_info!(), " }\n    ",
            "inverseRelations(first: 250, after: $inverse) @include(if: $withInverse) { nodes { ", relation_node!(), " } ", page_info!(), " }\n    ",
            "attachments(first: 250, after: $attachments) @include(if: $withAttachments) { nodes { title url } ", page_info!(), " }"
        )
    };
}

/// A relation as issue show has it: both issues, each with its title.
macro_rules! relation_node {
    () => {
        "id type issue { id identifier title } relatedIssue { id identifier title }"
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
    "dueDate",
    "estimate",
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

/// The issues a saved custom view matches, its own filter applied by Linear
/// and $filter on top. matches asks for two views, to catch a name two views
/// share; the issues come from the first one only (first multiplies cost).
pub const ISSUE_LIST_VIEW: &str = concat!(
    "query IssueListView($view: CustomViewFilter!, $filter: IssueFilter, $first: Int!, $after: String,\n  $orderBy: PaginationOrderBy, $sort: [IssueSortInput!]) {\n  matches: customViews(filter: $view, first: 2) { nodes { id name modelName } }\n  customViews(filter: $view, first: 1) { nodes { issues(filter: $filter, first: $first, after: $after, orderBy: $orderBy, sort: $sort) { nodes {",
    issue_row!(),
    " } ",
    page_info!(),
    " } } }\n}"
);

pub const VIEWS: &str = concat!(
    "query Views($filter: CustomViewFilter, $first: Int!, $after: String) {\n  customViews(filter: $filter, first: $first, after: $after) { nodes { id name description slugId modelName shared team { id key } owner { id name } updatedAt } ",
    page_info!(),
    " }\n}"
);

pub const ISSUE_LIST: &str = concat!(
    "query IssueList($filter: IssueFilter, $first: Int!, $after: String, $orderBy: PaginationOrderBy, $sort: [IssueSortInput!]) {\n  issues(filter: $filter, first: $first, after: $after, orderBy: $orderBy, sort: $sort) { nodes {",
    issue_row!(),
    " } ",
    page_info!(),
    " }\n}"
);

pub const ISSUE_SEARCH: &str = concat!(
    "query IssueSearch($term: String!, $filter: IssueFilter, $first: Int!, $after: String, $orderBy: PaginationOrderBy,\n  $includeComments: Boolean, $teamId: String) {\n  searchIssues(term: $term, filter: $filter, first: $first, after: $after, orderBy: $orderBy, includeComments: $includeComments,\n    teamId: $teamId) { nodes {",
    issue_row!(),
    " } ",
    page_info!(),
    " }\n}"
);

pub const ISSUE_SHOW: &str = concat!(
    "query IssueShow($id: String!, $comments: Int!, $withComments: Boolean!) {\n  issue(id: $id) {",
    issue_row!(),
    "\n    description dueDate estimate branchName startedAt completedAt canceledAt archivedAt\n    creator { id name email }\n    cycle { name startsAt endsAt }\n    project { status { name type } }\n    parent { identifier title }\n    ",
    show_connections!(""),
    "\n    comments(first: $comments) @include(if: $withComments) { nodes { id body createdAt url user { name email } parent { id } } }\n  }\n}"
);

/// The pages of issue show's collections after the first, each read only
/// while the one before said there is more.
pub const ISSUE_SHOW_MORE: &str = concat!(
    "query IssueShowMore($id: String!, $labels: String, $withLabels: Boolean!, $children: String, $withChildren: Boolean!,\n  $relations: String, $withRelations: Boolean!, $inverse: String, $withInverse: Boolean!,\n  $attachments: String, $withAttachments: Boolean!) {\n  issue(id: $id) {\n    ",
    show_connections!("more"),
    "\n  }\n}"
);

/// What issue create needs resolved, in one request: names to ids, and
/// whether an issue with the chosen id already exists.
pub const CREATE_RESOLVE: &str = concat!(
    "query CreateResolve($team: TeamFilter!, $id: ID!, $withID: Boolean!, $state: String!, $withState: Boolean!,\n  $email: String!, $withUser: Boolean!, $me: Boolean!, $labels: IssueLabelFilter!, $withLabels: Boolean!,\n  $project: String!, $withProject: Boolean!, $parent: IssueFilter!, $withParent: Boolean!,\n  $milestone: String!, $withMilestone: Boolean!, $delegate: String!, $withDelegate: Boolean!,\n  $stateType: String!, $withStateType: Boolean!,\n  $cycle: CycleFilter!, $withCycle: Boolean!) {\n  teams(filter: $team) { nodes { id key\n    states(filter: { name: { eqIgnoreCase: $state } }) @include(if: $withState) { nodes { id name } }\n    typed: states(filter: { type: { eq: $stateType } }) @include(if: $withStateType) { nodes { id name position } }\n    cycles(filter: $cycle, first: 2) @include(if: $withCycle) { nodes { id number } } } }\n  existing: issues(filter: { id: { eq: $id } }, includeArchived: true) @include(if: $withID) { nodes {",
    issue_row!(),
    " } }\n  users(filter: { email: { eqIgnoreCase: $email } }) @include(if: $withUser) { nodes { id name email } }\n  viewer @include(if: $me) { id name email app }\n  issueLabels(filter: $labels) @include(if: $withLabels) { nodes { id name isGroup team { id } } }\n  projects(filter: { name: { eqIgnoreCase: $project } }) @include(if: $withProject) { nodes { id name slugId } }\n  parent: issues(filter: $parent, first: 1) @include(if: $withParent) { nodes { id identifier } }\n  milestones: projectMilestones(filter: { name: { eqIgnoreCase: $milestone } }) @include(if: $withMilestone) { nodes { id name project { id name } } }\n  delegates: users(filter: { app: { eq: true }, or: [{ name: { eqIgnoreCase: $delegate } }, { displayName: { eqIgnoreCase: $delegate } }, { email: { eqIgnoreCase: $delegate } }] }) @include(if: $withDelegate) { nodes { id name } }\n}"
);

pub const ISSUE_CREATE: &str = concat!(
    "mutation IssueCreate($input: IssueCreateInput!) {\n  issueCreate(input: $input) { success issue {",
    issue_row!(),
    " } }\n}"
);

/// The issue as it is now, and what issue update needs resolved.
pub const UPDATE_RESOLVE: &str = "query UpdateResolve($id: String!, $state: String!, $withState: Boolean!,\n  $email: String!, $withUser: Boolean!, $me: Boolean!, $labels: IssueLabelFilter!, $withLabels: Boolean!,\n  $project: String!, $withProject: Boolean!, $parent: IssueFilter!, $withParent: Boolean!,\n  $milestone: String!, $withMilestone: Boolean!, $delegate: String!, $withDelegate: Boolean!,\n  $stateType: String!, $withStateType: Boolean!,\n  $cycle: CycleFilter!, $withCycle: Boolean!) {\n  issue(id: $id) { id identifier title description priority dueDate estimate updatedAt\n    team { id key states(filter: { name: { eqIgnoreCase: $state } }) @include(if: $withState) { nodes { id name } }\n    typed: states(filter: { type: { eq: $stateType } }) @include(if: $withStateType) { nodes { id name position } }\n    cycles(filter: $cycle, first: 2) @include(if: $withCycle) { nodes { id number } } }\n    state { id name } assignee { id email } project { id name } parent { id identifier }\n    projectMilestone { id } delegate { id } cycle { id }\n    labels(first: 100) { nodes { id name } } }\n  users(filter: { email: { eqIgnoreCase: $email } }) @include(if: $withUser) { nodes { id name email } }\n  viewer @include(if: $me) { id name email app }\n  issueLabels(filter: $labels) @include(if: $withLabels) { nodes { id name isGroup team { id } } }\n  projects(filter: { name: { eqIgnoreCase: $project } }) @include(if: $withProject) { nodes { id name slugId } }\n  parent: issues(filter: $parent, first: 1) @include(if: $withParent) { nodes { id identifier } }\n  milestones: projectMilestones(filter: { name: { eqIgnoreCase: $milestone } }) @include(if: $withMilestone) { nodes { id name project { id name } } }\n  delegates: users(filter: { app: { eq: true }, or: [{ name: { eqIgnoreCase: $delegate } }, { displayName: { eqIgnoreCase: $delegate } }, { email: { eqIgnoreCase: $delegate } }] }) @include(if: $withDelegate) { nodes { id name } }\n}";

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

/// What a team can use when a state or label name is not found.
pub const AVAILABLE: &str = "query Available($team: String!, $states: Boolean!, $labels: Boolean!) {\n  team(id: $team) {\n    states(first: 250) @include(if: $states) { nodes { name } }\n    labels(first: 250, filter: { isGroup: { eq: false } }) @include(if: $labels) { nodes { name } } }\n  issueLabels(first: 250, filter: { team: { null: true }, isGroup: { eq: false } }) @include(if: $labels) { nodes { name } }\n}";

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

pub const WATCH_CHANGED: &str = concat!(
    "query WatchChanged($filter: IssueFilter!, $first: Int!, $after: String) {\n  issues(filter: $filter, first: $first, after: $after, includeArchived: true, orderBy: updatedAt) { nodes { id } ",
    page_info!(),
    " }\n}"
);

pub const WATCH_CREATED: &str = "subscription WatchCreated { issueCreated { id } }";
pub const WATCH_UPDATED: &str = "subscription WatchUpdated { issueUpdated { id } }";
pub const WATCH_ARCHIVED: &str = "subscription WatchArchived { issueArchived { id } }";
pub const WATCH_UNARCHIVED: &str = "subscription WatchUnarchived { issueUnarchived { id } }";

pub const TEAMS: &str = concat!(
    "query Teams($first: Int!, $after: String) {\n  teams(first: $first, after: $after) { nodes { id key name description private } ",
    page_info!(),
    " }\n}"
);

/// A team's id, for an argument that takes one and not a filter.
pub const TEAM_FIND: &str =
    "query TeamFind($key: String!) {\n  teams(filter: { key: { eqIgnoreCase: $key } }) { nodes { id } }\n}";

pub const STATES: &str = concat!(
    "query States($filter: WorkflowStateFilter, $first: Int!, $after: String) {\n  workflowStates(filter: $filter, first: $first, after: $after) { nodes { id name type position color team { key } } ",
    page_info!(),
    " }\n}"
);

/// Labels by id: those an issue row's first 20 did not include.
pub const LABELS_BY_ID: &str = "query LabelsByID($ids: [ID!]!) {\n  issueLabels(filter: { id: { in: $ids } }, first: 250, includeArchived: true) { nodes { id name color } }\n}";

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
    "query Projects($filter: ProjectFilter, $first: Int!, $after: String) {\n  projects(filter: $filter, first: $first, after: $after) { nodes { id name slugId url startDate targetDate progress status { name type } lead { name email } } ",
    page_info!(),
    " }\n}"
);

/// project show: a project's documents after the first 250.
pub const PROJECT_DOCUMENTS: &str = concat!(
    "query ProjectDocuments($id: String!, $after: String) {\n  project(id: $id) { documents(first: 250, after: $after) { nodes { id title url } ",
    page_info!(),
    " } }\n}"
);

/// matches asks for two, so a name two projects share shows up as ambiguous;
/// the detail asks for one, since first multiplies the cost of everything
/// nested (as for cycles: two at 250 issues each is over Linear's 10,000).
pub const PROJECT_FIND: &str = concat!(
    "query ProjectFind($filter: ProjectFilter!, $issues: Int!, $withIssues: Boolean!) {\n  matches: projects(filter: $filter, first: 2) { nodes { id name slugId url } }\n  projects(filter: $filter, first: 1) { nodes {\n    id name slugId url description content startDate targetDate progress health createdAt updatedAt\n    status { id name type } lead { id name email }\n    teams(first: 20) { nodes { id key name } }\n    documents(first: 250) { nodes { id title url } ",
    page_info!(),
    " }\n    projectMilestones(first: 50) { nodes { id name targetDate progress } }\n    issues(first: $issues) @include(if: $withIssues) { nodes {",
    issue_row!(),
    " } ",
    page_info!(),
    " }\n  } }\n}"
);

/// project show --all-issues: a project's issues after the first page.
pub const PROJECT_ISSUES: &str = concat!(
    "query ProjectIssues($id: String!, $after: String) {\n  project(id: $id) { issues(first: 250, after: $after) { nodes {",
    issue_row!(),
    " } ",
    page_info!(),
    " } }\n}"
);

pub const PROJECT_UPDATE: &str = "mutation ProjectUpdate($id: String!, $input: ProjectUpdateInput!) {\n  projectUpdate(id: $id, input: $input) { success }\n}";

/// issue list's and watch's --project and --milestone, resolved to ids
/// together before the issues are read.
pub const PLACE_RESOLVE: &str = "query PlaceResolve($project: ProjectFilter!, $withProject: Boolean!,\n  $milestone: ProjectMilestoneFilter!, $withMilestone: Boolean!) {\n  projects(filter: $project, first: 50) @include(if: $withProject) { nodes { id name slugId url } }\n  projectMilestones(filter: $milestone, first: 50) @include(if: $withMilestone) { nodes { id name project { id name } } }\n}";

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
/// cycle show --all-issues: the cycle's issues after the first page.
pub const CYCLE_ISSUES: &str = concat!(
    "query CycleIssues($id: String!, $after: String) {\n  cycle(id: $id) { issues(first: 250, after: $after) { nodes {",
    issue_row!(),
    " } ",
    page_info!(),
    " } }\n}"
);

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
        ("IssueShowMore", ISSUE_SHOW_MORE),
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
        ("Available", AVAILABLE),
        ("RelationCreate", RELATION_CREATE),
        ("RelationDelete", RELATION_DELETE),
        ("Comment", COMMENT),
        ("CommentUpdate", COMMENT_UPDATE),
        ("CommentDelete", COMMENT_DELETE),
        ("Teams", TEAMS),
        ("TeamFind", TEAM_FIND),
        ("States", STATES),
        ("Labels", LABELS),
        ("LabelsByID", LABELS_BY_ID),
        ("Users", USERS),
        ("Projects", PROJECTS),
        ("PlaceResolve", PLACE_RESOLVE),
        ("Cycles", CYCLES),
        ("IssueListView", ISSUE_LIST_VIEW),
        ("Views", VIEWS),
        ("CycleShow", CYCLE_SHOW),
        ("CycleIssues", CYCLE_ISSUES),
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
        ("ProjectDocuments", PROJECT_DOCUMENTS),
        ("ProjectIssues", PROJECT_ISSUES),
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
