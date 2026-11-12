import assert from "node:assert/strict";
import test from "node:test";
import { act } from "react";
import { TARGET_LABELS } from "../src/computeTargets.ts";
import { m } from "../src/paraglide/messages.js";
import { find, renderApp, serveApp, settle, type } from "./render.mjs";

const saved = {
  host: "login", partition: "gpu", account: "lab", qos: null, timeLimit: null,
  cpusPerTask: null, mem: null, sbatchArgs: ["--rsc p=1:g=1"], requestOneTask: true,
  hosts: [{ host: "login" }],
};

const click = async (element) => {
  await act(async () => element.click());
  await settle();
};

const field = (label) => find("label", label).querySelector("input, textarea");

const routes = (save) => {
  let current = saved;
  return {
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "GET /api/settings/compute": () => ({ defaultBackend: null, defaultFlavor: null, targets: [{ id: "slurm", configured: true, enabled: true, summary: "" }] }),
    "GET /api/settings/slurm": () => current,
    "POST /api/settings/slurm": (init) => {
      const response = save(JSON.parse(init.body));
      if (!(response instanceof Response)) current = response;
      return response;
    },
  };
};

const openSlurm = async () => {
  await renderApp("/projects/p/settings/compute");
  await click(document.querySelector(`button[aria-label="${TARGET_LABELS.slurm()}"]`));
};

const oneTask = () => document.querySelector('button[role="switch"][aria-labelledby$="slurm-one-task"]');

test("Slurm settings save the QoS, sbatch options and the one-task switch", async () => {
  const posted = [];
  serveApp(routes((body) => {
    posted.push(body);
    return { ...saved, ...body, qos: body.qos || null };
  }));
  await openSlurm();

  assert.equal(field(m.settings_slurm_extra_options()).value, "--rsc p=1:g=1");
  assert.equal(oneTask().getAttribute("aria-checked"), "true");
  assert.equal(find("button", m.common_save()).disabled, true);

  await type(field(m.settings_slurm_qos()), " acc_ehpc ");
  await type(field(m.settings_slurm_extra_options()), "--rsc p=1:g=1\n\n  #SBATCH --gres=gpu:1 ");
  await click(oneTask());
  await click(find("button", m.common_save()));

  assert.equal(posted.length, 1);
  assert.equal(posted[0].qos, "acc_ehpc");
  assert.deepEqual(posted[0].sbatchArgs, ["--rsc p=1:g=1", "#SBATCH --gres=gpu:1"]);
  assert.equal(posted[0].requestOneTask, false);
  assert.equal(find("button", m.common_save()).disabled, true);
});

test("Slurm settings show each rejection under its field until it is edited", async () => {
  const sbatchErrors = [
    "--output=/scratch/me/log sets --output, which OpenResearch sets itself to log and track the job. Remove that line.",
    "-p debug sets --partition, which Partition already sets. Remove that line or clear Partition.",
  ];
  serveApp(routes(() => Response.json(
    { error: [...sbatchErrors, "QoS takes one Slurm value without spaces, such as acc_ehpc."].join("\n"), fields: { sbatchArgs: sbatchErrors, qos: ["QoS takes one Slurm value without spaces, such as acc_ehpc."] } },
    { status: 400 },
  )));
  await openSlurm();

  await type(field(m.settings_slurm_qos()), "a b");
  await type(field(m.settings_slurm_extra_options()), "--output=/scratch/me/log\n-p debug");
  await click(find("button", m.common_save()));

  const errorsFor = (input) =>
    (input.getAttribute("aria-describedby") ?? "").split(" ").map((id) => document.getElementById(id)?.textContent ?? "").join(" ");
  const sbatch = field(m.settings_slurm_extra_options());
  assert.equal(sbatch.getAttribute("aria-invalid"), "true");
  for (const message of sbatchErrors) assert.ok(errorsFor(sbatch).includes(message), message);
  assert.ok(errorsFor(field(m.settings_slurm_qos())).includes("QoS takes one Slurm value"));
  assert.equal(document.querySelector(".error"), null);

  await type(field(m.settings_slurm_qos()), "acc_ehpc");
  assert.equal(field(m.settings_slurm_qos()).getAttribute("aria-invalid"), null);
  assert.ok(errorsFor(sbatch).includes(sbatchErrors[0]));
});

test("Slurm settings from an older orx hide the launch options it can't apply", async () => {
  const { qos, sbatchArgs, requestOneTask, ...older } = saved;
  let posted = null;
  serveApp({
    "GET /api/projects": () => ({ projects: [{ id: "p", name: "Project" }] }),
    "GET /api/settings/compute": () => ({ defaultBackend: null, defaultFlavor: null, targets: [{ id: "slurm", configured: true, enabled: true, summary: "" }] }),
    "GET /api/settings/slurm": () => older,
    "POST /api/settings/slurm": (init) => {
      posted = JSON.parse(init.body);
      return { ...older, ...posted };
    },
  });
  await renderApp("/projects/p/settings/compute");
  await click(document.querySelector(`button[aria-label="${TARGET_LABELS.slurm()}"]`));

  assert.equal(find("label", m.settings_slurm_qos()), null);
  assert.equal(find("label", m.settings_slurm_extra_options()), null);
  assert.equal(oneTask(), null);
  await type(field(m.settings_page_account()), "other");
  await click(find("button", m.common_save()));
  assert.equal(posted.account, "other");
  assert.equal("sbatchArgs" in posted, false);
});
