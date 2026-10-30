import { m } from "../paraglide/messages.js";
import { type DialogContentProps } from "../dialog";
import { X } from "lucide-react";
import { BrandMark } from "./Wordmark";
import { Button, DialogContent, DialogTitle, IconButton, Spinner } from "./ui";

export function DemoWelcomeModal({ confirm, isDisabled, errorMessage }: DialogContentProps<"explore_demo" | "create_project" | "dismiss">) {
  return <DialogContent className="relative w-110">
      <IconButton
        className="absolute end-3.5 top-3.5"
        aria-label={m.tour_close()}
        onClick={() => confirm("dismiss")}
        disabled={isDisabled}
      >
        <X size={16} />
      </IconButton>
      <div className="mb-5 flex items-center gap-3 pe-8">
        <span className="block h-9 w-9 shrink-0 [&_svg]:block [&_svg]:h-full [&_svg]:w-full">
          <BrandMark />
        </span>
        <div>
          <div className="mb-0.5 text-xs font-medium tracking-wider text-subtext uppercase">
            {m.tour_demo_project()}
          </div>
          <DialogTitle
            className="m-0 text-2xl leading-tight tracking-tight"
          >
            {m.tour_welcome_to_open_research()}
          </DialogTitle>
        </div>
      </div>
      <div className="text-base leading-relaxed text-text [&_p]:m-0 [&_p_+_p]:mt-3">
        <p dir="auto">
          {m.tour_this_is_a_demo_project_showing_how_open()}{" "}
          <a
            dir="ltr"
            href="https://github.com/karpathy/nanochat"
            target="_blank"
            rel="noreferrer"
            className="font-medium text-accent-blue-strong underline decoration-border-strong underline-offset-3 hover:decoration-current"
          >
            {m.tour_nanochat()}
          </a>{m.tour_a_repo_for_training_a_mini_gpt_from()}
        </p>
        <p dir="auto">
          {m.tour_look_through_the_agent_conversations_experiments_runs_and()}
        </p>
      </div>
      {errorMessage && <p role="alert" className="mt-3 mb-0 text-sm text-accent-red">{errorMessage}</p>}
      <div className="mt-6 flex flex-wrap items-center justify-end gap-2.5">
        <Button
          onClick={() => confirm("create_project")}
          disabled={isDisabled}
        >
          {m.tour_create_a_new_project()}
        </Button>
        <Button variant="primary"
          onClick={() => confirm("explore_demo")}
          disabled={isDisabled}
        >
          {isDisabled && <Spinner />}
          {isDisabled ? m.common_saving() : m.tour_run_demo_experiment()}
        </Button>
      </div>
    </DialogContent>;
}
