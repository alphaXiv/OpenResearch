import { memo, type ReactNode } from "react";
import { autoDir } from "../../../i18n";
import { m } from "../../../paraglide/messages.js";
import { getLocale } from "../../../paraglide/runtime.js";
import type { Tone } from "../../../runMonitor/model";
import type { RunMonitor, ViewBlock } from "../../../runMonitor/types";
import { Boundary } from "../../ui/Boundary";
import { Bars } from "./Bars";
import { Cells } from "./Cells";
import { Chart } from "./Chart";
import { Legend } from "./Legend";
import { Media } from "./Media";
import { seriesOf } from "./series";
import { midSentence, Stats } from "./Stats";
import { Table } from "./Table";
import { Text } from "./Text";

type BlockSize = "strip" | "page";
type BlockProps = { monitor: RunMonitor; block: ViewBlock; size: BlockSize; tone: Tone; live: boolean };

const KINDS = ["chart", "stats", "table", "bars", "cells", "media", "text"] as const;

/** What a block draws, when this build knows it; a newer server's kinds are skipped. */
const kindOf = (block: ViewBlock) => KINDS.find((kind) => kind in block) ?? null;

/** A block's heading on a page: the agent's title, else one orx words from what the block shows. */
function titleOf(block: ViewBlock, monitor: RunMonitor): string | null {
  if (block.title) return block.title;
  const label = (name: string) => monitor.view.signals[name]?.label ?? name;
  if ("chart" in block) {
    const list = (names: string[]) => new Intl.ListFormat(getLocale(), { type: "conjunction" }).format(names.map((name, index) => index === 0 ? label(name) : midSentence(label(name))));
    if (block.from === undefined) return list(block.chart);
    const ys = block.series?.length ? block.series.map((one) => one.y) : block.chart;
    const x = block.series?.[0]?.x ?? block.x ?? "step";
    return m.run_chart_against({ y: list([...new Set(ys)]), x: midSentence(label(x)) });
  }
  if ("table" in block) {
    const name = block.table.replaceAll("_", " ");
    return block.vs ? name.charAt(0).toLocaleUpperCase(getLocale()) + name.slice(1) : m.run_block_latest({ table: name });
  }
  if ("bars" in block) return m.run_block_accuracy_by({ key: label(block.by) });
  if ("cells" in block) return m.run_block_cells();
  if ("media" in block) return m.run_block_media();
  return null;
}

/** A block that throws says so in its place and the rest of the view keeps drawing. A newer revision tries it again. */
export function BlockBoundary({ title, rev, children }: { title: string; rev: number; children: ReactNode }) {
  const failed = <p className="m-0 text-xs text-ink-subtext">{m.run_block_failed({ title: autoDir(title) })}</p>;
  return <Boundary name="run view block" rev={rev} fallback={failed}>{children}</Boundary>;
}

/** Stats and text are the page's alone: at smaller sizes the headline is the number and the sentence the words. */
export function drawsAt(block: ViewBlock, size: BlockSize): boolean {
  const kind = kindOf(block);
  return kind !== null && (size === "page" || (kind !== "stats" && kind !== "text"));
}

function Body({ monitor, block, size, tone, live, title }: BlockProps & { title: string }) {
  if ("chart" in block) {
    return <Chart monitor={monitor} block={block} title={title} size={size} live={live} height={block.chart.length > 1 || block.from !== undefined ? 200 : 160} />;
  }
  if ("table" in block) return <Table monitor={monitor} block={block} size={size} live={live} title={title} />;
  if ("bars" in block) return <Bars monitor={monitor} block={block} size={size} live={live} />;
  if ("cells" in block) return <Cells rows={Object.hasOwn(monitor.rows, block.cells) ? monitor.rows[block.cells] : []} size={size} live={live} />;
  if ("media" in block) return <Media gallery={monitor.media[block.media]} glob={block.media} last={block.last} runId={monitor.runId} live={live} page={size === "page"} />;
  if ("stats" in block) return <Stats monitor={monitor} names={block.stats} vs={block.vs} live={live} />;
  return <Text monitor={monitor} template={block.text} tone={tone} />;
}

/**
 * One block of a run's view at one size: on a page with a heading, in a strip bare. A kind this build doesn't know
 * draws nothing. It redraws when its data does, never on the clock of the page around it.
 */
export const Block = memo(function Block(props: BlockProps) {
  if (!drawsAt(props.block, props.size)) return null;
  let title: string | null = null;
  try {
    title = titleOf(props.block, props.monitor);
  } catch {
    // A block too malformed to name is drawn, and fails, under the boundary below.
  }
  const named = title ?? m.run_block_this();
  const body = <Body {...props} title={named} />;
  return (
    <BlockBoundary title={named} rev={props.monitor.rev}>
      {props.size === "strip" || title === null ? body : (
        <section aria-label={title}>
          <div className="mb-2.5 flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
            <h3 className="m-0 text-sm font-medium text-text">{autoDir(title)}</h3>
            {"chart" in props.block && <Legend series={seriesOf(props.monitor, props.block)} row />}
          </div>
          {body}
        </section>
      )}
    </BlockBoundary>
  );
});
