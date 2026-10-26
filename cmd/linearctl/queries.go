package main

import "strings"

// Every GraphQL document linearctl sends. queries_test.go validates each
// against the repository's graphql/schema.graphql, so a field Linear renames
// fails the tests rather than a user. Update the schema with `make schema`.

// issueRow is what a list shows for an issue. Search results are a different
// type with the same fields, so it is a field list, not a fragment.
//
// Nested objects carry their id, so a caller can join them with state, label,
// user, team, project and cycle list: names are not unique across teams.
const issueRow = `
	id identifier title priority priorityLabel url createdAt updatedAt
	state { id name type color }
	assignee { id name email }
	team { id key }
	project { id name }
	cycle { id number }
	projectMilestone { id name }
	labels(first: 20) { nodes { id name color } }`

// issueRowFields are the keys of an issueRow item, for --fields.
var issueRowFields = []string{"id", "identifier", "title", "priority", "priorityLabel", "url", "createdAt",
	"updatedAt", "state", "assignee", "team", "project", "cycle", "projectMilestone", "labels"}

const pageInfo = `pageInfo { hasNextPage endCursor }`

var (
	qViewer = `query Viewer { viewer { id name email } organization { name urlKey } }`

	qIssueList = `query IssueList($filter: IssueFilter, $first: Int!, $after: String, $orderBy: PaginationOrderBy, $sort: [IssueSortInput!]) {
  issues(filter: $filter, first: $first, after: $after, orderBy: $orderBy, sort: $sort) { nodes {` + issueRow + ` } ` + pageInfo + ` }
}`

	qIssueSearch = `query IssueSearch($term: String!, $filter: IssueFilter, $first: Int!, $after: String, $orderBy: PaginationOrderBy) {
  searchIssues(term: $term, filter: $filter, first: $first, after: $after, orderBy: $orderBy) { nodes {` + issueRow + ` } ` + pageInfo + ` }
}`

	qIssueShow = `query IssueShow($id: String!, $comments: Int!, $withComments: Boolean!) {
  issue(id: $id) {` + issueRow + `
    description dueDate estimate branchName startedAt completedAt canceledAt archivedAt
    creator { name email }
    parent { identifier title }
    children(first: 50) { nodes { identifier title state { name type } } }
    relations(first: 50) { nodes { id type relatedIssue { identifier title } } }
    inverseRelations(first: 50) { nodes { id type issue { identifier title } } }
    attachments(first: 50) { nodes { title url } }
    comments(first: $comments) @include(if: $withComments) { nodes { id body createdAt url user { name email } parent { id } } }
  }
}`

	// What issue create needs resolved, in one request: names to ids, and
	// whether an issue with the chosen id already exists.
	qCreateResolve = `query CreateResolve($team: String!, $id: ID!, $withID: Boolean!, $state: String!, $withState: Boolean!,
  $email: String!, $withUser: Boolean!, $me: Boolean!, $labels: [String!]!, $withLabels: Boolean!,
  $project: String!, $withProject: Boolean!, $parent: IssueFilter!, $withParent: Boolean!,
  $milestone: String!, $withMilestone: Boolean!) {
  teams(filter: { key: { eqIgnoreCase: $team } }) { nodes { id key
    states(filter: { name: { eqIgnoreCase: $state } }) @include(if: $withState) { nodes { id name } } } }
  existing: issues(filter: { id: { eq: $id } }, includeArchived: true) @include(if: $withID) { nodes {` + issueRow + ` } }
  users(filter: { email: { eqIgnoreCase: $email } }) @include(if: $withUser) { nodes { id name email } }
  viewer @include(if: $me) { id name email }
  issueLabels(filter: { name: { in: $labels } }) @include(if: $withLabels) { nodes { id name team { id } } }
  projects(filter: { name: { eqIgnoreCase: $project } }) @include(if: $withProject) { nodes { id name } }
  parent: issues(filter: $parent, first: 1) @include(if: $withParent) { nodes { id identifier } }
  milestones: projectMilestones(filter: { name: { eqIgnoreCase: $milestone } }) @include(if: $withMilestone) { nodes { id name project { id name } } }
}`

	qIssueCreate = `mutation IssueCreate($input: IssueCreateInput!) {
  issueCreate(input: $input) { success issue {` + issueRow + ` } }
}`

	// The issue as it is now, and what issue update needs resolved.
	qUpdateResolve = `query UpdateResolve($id: String!, $state: String!, $withState: Boolean!,
  $email: String!, $withUser: Boolean!, $me: Boolean!, $labels: [String!]!, $withLabels: Boolean!,
  $project: String!, $withProject: Boolean!, $parent: IssueFilter!, $withParent: Boolean!,
  $milestone: String!, $withMilestone: Boolean!) {
  issue(id: $id) { id identifier title description priority dueDate estimate
    team { id key states(filter: { name: { eqIgnoreCase: $state } }) @include(if: $withState) { nodes { id name } } }
    state { id name } assignee { id email } project { id name } parent { id identifier }
    projectMilestone { id }
    labels(first: 100) { nodes { id name } } }
  users(filter: { email: { eqIgnoreCase: $email } }) @include(if: $withUser) { nodes { id name email } }
  viewer @include(if: $me) { id name email }
  issueLabels(filter: { name: { in: $labels } }) @include(if: $withLabels) { nodes { id name team { id } } }
  projects(filter: { name: { eqIgnoreCase: $project } }) @include(if: $withProject) { nodes { id name } }
  parent: issues(filter: $parent, first: 1) @include(if: $withParent) { nodes { id identifier } }
  milestones: projectMilestones(filter: { name: { eqIgnoreCase: $milestone } }) @include(if: $withMilestone) { nodes { id name project { id name } } }
}`

	qIssueUpdate = `mutation IssueUpdate($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) { success issue {` + issueRow + ` } }
}`

	qIssueArchived = `query IssueArchived($id: String!) { issue(id: $id) { id identifier title url archivedAt } }`

	qIssueArchive = `mutation IssueArchive($id: String!) { issueArchive(id: $id) { success } }`

	qIssueUnarchive = `mutation IssueUnarchive($id: String!) { issueUnarchive(id: $id) { success } }`

	commentRow = `id body createdAt url user { name email } parent { id }`

	qCommentList = `query CommentList($id: String!, $first: Int!, $after: String) {
  issue(id: $id) { comments(first: $first, after: $after) { nodes { ` + commentRow + ` } ` + pageInfo + ` } }
}`

	qCommentResolve = `query CommentResolve($issue: String!, $id: ID!) {
  issue(id: $issue) { id identifier }
  comments(filter: { id: { eq: $id } }) { nodes { ` + commentRow + ` } }
}`

	qCommentCreate = `mutation CommentCreate($input: CommentCreateInput!) {
  commentCreate(input: $input) { success comment { ` + commentRow + ` } }
}`

	qComment = `query Comment($id: String!) { comment(id: $id) { ` + commentRow + ` } }`

	qCommentUpdate = `mutation CommentUpdate($id: String!, $input: CommentUpdateInput!) {
  commentUpdate(id: $id, input: $input) { success comment { ` + commentRow + ` } }
}`

	qCommentDelete = `mutation CommentDelete($id: String!) { commentDelete(id: $id) { success } }`

	// Both issues by filter (a missing one is an empty list, not an error),
	// with the relations stored from and to the first.
	qRelationResolve = `query RelationResolve($a: IssueFilter!, $b: IssueFilter!) {
  a: issues(filter: $a, first: 1, includeArchived: true) { nodes { id identifier
    relations(first: 250) { nodes { id type relatedIssue { id identifier } } }
    inverseRelations(first: 250) { nodes { id type issue { id identifier } } } } }
  b: issues(filter: $b, first: 1, includeArchived: true) { nodes { id identifier } }
}`

	qRelationCreate = `mutation RelationCreate($input: IssueRelationCreateInput!) {
  issueRelationCreate(input: $input) { success issueRelation { id } }
}`

	qRelationDelete = `mutation RelationDelete($id: String!) { issueRelationDelete(id: $id) { success } }`

	qTeams = `query Teams($first: Int!, $after: String) {
  teams(first: $first, after: $after) { nodes { id key name description private } ` + pageInfo + ` }
}`

	qStates = `query States($filter: WorkflowStateFilter, $first: Int!, $after: String) {
  workflowStates(filter: $filter, first: $first, after: $after) { nodes { id name type position color team { key } } ` + pageInfo + ` }
}`

	qLabels = `query Labels($filter: IssueLabelFilter, $first: Int!, $after: String) {
  issueLabels(filter: $filter, first: $first, after: $after) { nodes { id name color isGroup team { key } parent { name } } ` + pageInfo + ` }
}`

	qUsers = `query Users($filter: UserFilter, $first: Int!, $after: String) {
  users(filter: $filter, first: $first, after: $after) { nodes { id name displayName email active admin guest } ` + pageInfo + ` }
}`

	qProjects = `query Projects($filter: ProjectFilter, $first: Int!, $after: String) {
  projects(filter: $filter, first: $first, after: $after) { nodes { id name url targetDate status { name type } lead { name email } } ` + pageInfo + ` }
}`

	qMilestones = `query Milestones($filter: ProjectMilestoneFilter, $first: Int!, $after: String) {
  projectMilestones(filter: $filter, first: $first, after: $after) { nodes { id name targetDate progress project { id name } } ` + pageInfo + ` }
}`

	qCycles = `query Cycles($filter: CycleFilter, $first: Int!, $after: String) {
  cycles(filter: $filter, first: $first, after: $after) { nodes { id number name startsAt endsAt isActive isNext team { key } } ` + pageInfo + ` }
}`
)

// documents is every document above, by name, for the schema test.
func documents() map[string]string {
	return map[string]string{
		"Viewer": qViewer, "IssueList": qIssueList, "IssueSearch": qIssueSearch, "IssueShow": qIssueShow,
		"CreateResolve": qCreateResolve, "IssueCreate": qIssueCreate, "UpdateResolve": qUpdateResolve,
		"IssueUpdate": qIssueUpdate, "IssueArchived": qIssueArchived, "IssueArchive": qIssueArchive, "IssueUnarchive": qIssueUnarchive,
		"CommentList": qCommentList, "CommentResolve": qCommentResolve, "CommentCreate": qCommentCreate,
		"RelationResolve": qRelationResolve, "RelationCreate": qRelationCreate, "RelationDelete": qRelationDelete,
		"Comment": qComment, "CommentUpdate": qCommentUpdate, "CommentDelete": qCommentDelete,
		"Teams": qTeams, "States": qStates, "Labels": qLabels, "Users": qUsers, "Projects": qProjects,
		"Cycles": qCycles, "Milestones": qMilestones,
	}
}

// opName is a document's operation name, which tests use to answer it.
func opName(doc string) string {
	f := strings.Fields(doc)
	if len(f) < 2 {
		return ""
	}
	name, _, _ := strings.Cut(f[1], "(")
	return name
}
