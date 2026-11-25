import { test } from "node:test";
import assert from "node:assert/strict";
import {
  SCRATCH,
  defaultFolder,
  groupOrder,
  nameProblem,
  overlapProblem,
  projectAt,
  projectColor,
  projectOf,
  readLinks,
  readProjects,
  scratchProject,
  sidebarProjects,
} from "../src/projects.ts";

const projects = readProjects([
  { id: "web", name: "web site", folder: "~/p/web", path: "/h/p/web", color: "teal", missing: false },
  { id: "kelp", name: "Kelp", folder: "~/p/kelp", path: "/h/p/kelp", color: "#123456", missing: true },
  { id: 3, name: "broken" },
]);
const scratch = scratchProject("~/scratch", "/h/scratch");

test("projects and links are read defensively", () => {
  assert.equal(projects.length, 2);
  assert.equal(projects[1].missing, true);
  assert.deepEqual(readLinks({ panes: { 3: "kelp", x: "web", 4: 5 } }), { 3: "kelp" });
  assert.deepEqual(readLinks(null), {});
});

test("a terminal without a link, or linked to a gone project, is Scratch's", () => {
  const links = { 1: "kelp", 2: "gone" };
  assert.equal(projectOf(1, links, projects), "kelp");
  assert.equal(projectOf(2, links, projects), SCRATCH);
  assert.equal(projectOf(3, links, projects), SCRATCH);
});

test("the sidebar lists projects by name, Scratch last, and terminals follow", () => {
  const order = sidebarProjects(projects, scratch).map((p) => p.id);
  assert.deepEqual(order, ["kelp", "web", SCRATCH]);
  const links = { 1: "web", 3: "kelp", 4: "web" };
  const panes = [1, 2, 3, 4].map((id) => ({ id }));
  assert.deepEqual(
    groupOrder(panes, order, (id) => projectOf(id, links, projects)).map((p) => p.id),
    [3, 1, 4, 2],
  );
});

test("folders map to projects, and overlaps are refused", () => {
  assert.equal(projectAt("/h/p/web/src", projects)?.id, "web");
  assert.equal(projectAt("/h/p/webby", projects), undefined);
  assert.match(overlapProblem("/h/p/web", projects), /already the folder/);
  assert.match(overlapProblem("/h/p/web/a", projects), /inside the folder of “web site”/);
  assert.match(overlapProblem("/h/p", projects), /contains/);
  assert.equal(overlapProblem("/h/q", projects), null);
});

test("names, colours and default folders", () => {
  assert.equal(nameProblem("New", projects), null);
  assert.match(nameProblem("KELP", projects), /already named/);
  assert.equal(nameProblem("KELP", projects, "kelp"), null);
  assert.match(nameProblem("a/b", projects), /“\/”/);
  assert.match(nameProblem("scratch", projects), /built-in/);
  assert.match(nameProblem(" x", projects), /spaces/);
  assert.equal(projectColor(projects[0]), "#3fb8b0");
  assert.equal(projectColor(projects[1]), "#123456");
  assert.equal(projectColor(scratch), "");
  assert.equal(defaultFolder("~/projects/", "Kelp"), "~/projects/Kelp");
});
