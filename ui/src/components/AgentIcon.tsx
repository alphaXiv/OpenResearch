import { useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { Popover } from "@base-ui/react/popover";
import type { Gemoji } from "gemoji";
import {
  Apple, Bot, BoxIcon, Brain, BriefcaseBusiness, Building2, Check, CodeXml, Flag, Flame, FlaskConical, Ghost,
  GraduationCap, Heart, House, Leaf, Lightbulb, Moon, Pencil, Plane, Plus, Rocket, Search, Smile, Sparkles,
  SquareTerminal, Star, Trophy, User, Zap, type LucideIcon,
} from "lucide-react";
import type { AgentIcon, AgentIconColor } from "../api";
import { m } from "../paraglide/messages.js";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { Button, Input, SEGMENTED_CLASS_NAME } from "./ui";
import { cn } from "./ui/cn";

/** The plain outline a mark sits in without a colour, its logo in its own colours. */
const PLAIN = { tile: "border-border text-text", cell: "aria-checked:border-border aria-checked:bg-hover-muted aria-checked:text-text" };

/** A provider's own logo in the plain outline: every agent's mark until the user picks another. */
export const BRAND_ICON: AgentIcon = { brand: true };

/** Whole class strings so Tailwind sees them: `tile` tints a glyph or emoji, `solid` fills behind an image, `cell` marks the picked cell. */
const COLORS: Record<AgentIconColor, { label: () => string; tile: string; solid: string; cell: string }> = {
  gray: { label: m.icon_color_gray, tile: "border-muted/30 bg-muted/12 text-subtext", solid: "bg-muted", cell: "aria-checked:border-muted/40 aria-checked:bg-muted/14 aria-checked:text-text" },
  red: { label: m.icon_color_red, tile: "border-accent-red/25 bg-accent-red/12 text-accent-red", solid: "bg-accent-red", cell: "aria-checked:border-accent-red/40 aria-checked:bg-accent-red/14 aria-checked:text-accent-red" },
  orange: { label: m.icon_color_orange, tile: "border-accent-orange/25 bg-accent-orange/12 text-accent-orange", solid: "bg-accent-orange", cell: "aria-checked:border-accent-orange/40 aria-checked:bg-accent-orange/14 aria-checked:text-accent-orange" },
  amber: { label: m.icon_color_amber, tile: "border-accent-amber/25 bg-accent-amber/12 text-accent-amber", solid: "bg-accent-amber", cell: "aria-checked:border-accent-amber/40 aria-checked:bg-accent-amber/14 aria-checked:text-accent-amber" },
  green: { label: m.icon_color_green, tile: "border-accent-green/25 bg-accent-green/12 text-accent-green", solid: "bg-accent-green", cell: "aria-checked:border-accent-green/40 aria-checked:bg-accent-green/14 aria-checked:text-accent-green" },
  teal: { label: m.icon_color_teal, tile: "border-accent-teal/25 bg-accent-teal/12 text-accent-teal", solid: "bg-accent-teal", cell: "aria-checked:border-accent-teal/40 aria-checked:bg-accent-teal/14 aria-checked:text-accent-teal" },
  blue: { label: m.icon_color_blue, tile: "border-accent-blue/25 bg-accent-blue/12 text-accent-blue", solid: "bg-accent-blue", cell: "aria-checked:border-accent-blue/40 aria-checked:bg-accent-blue/14 aria-checked:text-accent-blue" },
  indigo: { label: m.icon_color_indigo, tile: "border-accent-indigo/25 bg-accent-indigo/12 text-accent-indigo", solid: "bg-accent-indigo", cell: "aria-checked:border-accent-indigo/40 aria-checked:bg-accent-indigo/14 aria-checked:text-accent-indigo" },
  purple: { label: m.icon_color_purple, tile: "border-accent-purple/25 bg-accent-purple/12 text-accent-purple", solid: "bg-accent-purple", cell: "aria-checked:border-accent-purple/40 aria-checked:bg-accent-purple/14 aria-checked:text-accent-purple" },
  pink: { label: m.icon_color_pink, tile: "border-accent-pink/25 bg-accent-pink/12 text-accent-pink", solid: "bg-accent-pink", cell: "aria-checked:border-accent-pink/40 aria-checked:bg-accent-pink/14 aria-checked:text-accent-pink" },
};

/** Stored by id, so an id once offered keeps its glyph. */
const GLYPHS: [id: string, label: () => string, Icon: LucideIcon][] = [
  ["briefcase-business", m.icon_glyph_work, BriefcaseBusiness],
  ["house", m.icon_glyph_home, House],
  ["heart", m.icon_glyph_personal, Heart],
  ["user", m.icon_glyph_person, User],
  ["sparkles", m.icon_glyph_sparkles, Sparkles],
  ["flask-conical", m.icon_glyph_lab, FlaskConical],
  ["rocket", m.icon_glyph_rocket, Rocket],
  ["zap", m.icon_glyph_bolt, Zap],
  ["brain", m.icon_glyph_brain, Brain],
  ["bot", m.icon_glyph_bot, Bot],
  ["code-xml", m.icon_glyph_code, CodeXml],
  ["square-terminal", m.icon_glyph_terminal, SquareTerminal],
  ["graduation-cap", m.icon_glyph_school, GraduationCap],
  ["building-2", m.icon_glyph_office, Building2],
  ["star", m.icon_glyph_star, Star],
  ["flame", m.icon_glyph_flame, Flame],
  ["leaf", m.icon_glyph_leaf, Leaf],
  ["moon", m.icon_glyph_moon, Moon],
  ["ghost", m.icon_glyph_ghost, Ghost],
];

const TILE_SIZES = {
  xs: { box: "size-5 rounded-md text-xs", mark: 13 },
  md: { box: "size-8 rounded-lg text-lg", mark: 18 },
  lg: { box: "size-11 rounded-xl text-2xl", mark: 24 },
} as const;

/** A mark the picker offers first: the provider's own logo, drawn at the size asked. */
export type Logo = (size: number) => ReactNode;

function IconMark({ icon, brand, size }: { icon: AgentIcon; brand: Logo; size: number }) {
  if ("emoji" in icon) return <span className="leading-none">{icon.emoji}</span>;
  if ("image" in icon) return <img alt="" draggable={false} className="size-full object-cover" src={`data:${icon.image.type};base64,${icon.image.data}`} />;
  if ("brand" in icon) return brand(size);
  // A glyph another version stored and this one doesn't draw is a box.
  const Glyph = GLYPHS.find(([id]) => id === icon.glyph)?.[2] ?? BoxIcon;
  return <Glyph size={size} aria-hidden="true" />;
}

/** An agent's mark in its tile: on its colour, or in a plain outline. */
export function AgentIconTile({ icon, brand, size = "md", outline = true }: {
  icon: AgentIcon;
  brand: Logo;
  size?: keyof typeof TILE_SIZES;
  /** Whether an uncoloured icon draws its tile's border; without it, its mark grows into the space the border framed. */
  outline?: boolean;
}) {
  const color = icon.color && COLORS[icon.color];
  const bare = !color && !outline;
  const plain = bare ? cn(PLAIN.tile, "border-transparent") : PLAIN.tile;
  return (
    <span aria-hidden="true" className={cn("grid shrink-0 place-items-center overflow-hidden border", TILE_SIZES[size].box, !color ? plain : "image" in icon ? cn("border-transparent", color.solid) : color.tile)}>
      <IconMark icon={icon} brand={brand} size={TILE_SIZES[size].mark + (bare ? 4 : 0)} />
    </span>
  );
}

const GRID = "grid grid-cols-[repeat(auto-fill,minmax(2rem,1fr))] gap-1";
const CELL = "grid h-8.5 place-items-center rounded-lg border border-transparent text-subtext outline-none hover:bg-hover-muted hover:text-text focus-visible:outline-2 focus-visible:outline-focus";

/** Icons with the provider's logo first, Emoji and Image tabs, over a colour row. */
function AgentIconPicker({ value, brand, onChange }: { value: AgentIcon; brand: Logo; onChange: (icon: AgentIcon) => void }) {
  const [tab, setTab] = useState<"icons" | "emoji" | "image">(() => "emoji" in value ? "emoji" : "image" in value ? "image" : "icons");
  const [imageError, setImageError] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const color = value.color;
  const cell = cn(CELL, (color && COLORS[color] || PLAIN).cell);
  const tint = (next?: AgentIconColor) => {
    const { color: _, ...mark } = value;
    onChange(next ? { ...mark, color: next } : mark);
  };

  return (
    <div className="grid gap-3">
      <div className={cn(SEGMENTED_CLASS_NAME, "flex w-full")} role="group" aria-label={m.icon_kind()}>
        {(["icons", "emoji", "image"] as const).map((id) => (
          <button key={id} type="button" aria-pressed={tab === id} className={cn("flex-1 justify-center", tab === id && "active")} onClick={() => setTab(id)}>
            {id === "icons" ? m.icon_tab_icons() : id === "emoji" ? m.icon_tab_emoji() : m.icon_tab_image()}
          </button>
        ))}
      </div>
      <div role="radiogroup" aria-label={m.icon_color_label()} className="flex justify-between">
        <button
          type="button"
          role="radio"
          aria-checked={!color}
          aria-label={m.icon_color_none()}
          className="grid size-5 place-items-center rounded-full border border-border text-text outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
          onClick={() => tint()}
        >
          {!color && <Check size={13} strokeWidth={3} aria-hidden="true" />}
        </button>
        {(Object.keys(COLORS) as AgentIconColor[]).map((entry) => (
          <button
            key={entry}
            type="button"
            role="radio"
            aria-checked={entry === color}
            aria-label={COLORS[entry].label()}
            className={cn("grid size-5 place-items-center rounded-full text-white outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus", COLORS[entry].solid)}
            onClick={() => tint(entry)}
          >
            {entry === color && <Check size={13} strokeWidth={3} aria-hidden="true" />}
          </button>
        ))}
      </div>
      {tab === "icons" ? (
        <div role="radiogroup" aria-label={m.icon_tab_icons()} className={GRID}>
          <button type="button" role="radio" aria-checked={"brand" in value} aria-label={m.icon_provider_logo()} className={cell} onClick={() => onChange({ brand: true, color })}>
            {brand(18)}
          </button>
          {GLYPHS.map(([id, label, Glyph]) => (
            <button key={id} type="button" role="radio" aria-checked={"glyph" in value && value.glyph === id} aria-label={label()} className={cell} onClick={() => onChange({ glyph: id, color })}>
              <Glyph size={18} aria-hidden="true" />
            </button>
          ))}
        </div>
      ) : tab === "emoji" ? (
        <EmojiPicker value={"emoji" in value ? value.emoji : null} cell={cell} onPick={(emoji) => onChange({ emoji, color })} />
      ) : (
        <div className="flex items-center gap-3.5">
          {"image" in value ? (
            <AgentIconTile icon={value} brand={brand} size="lg" />
          ) : (
            <span aria-hidden="true" className="grid size-11 place-items-center rounded-xl border border-dashed border-border text-muted"><Plus size={20} /></span>
          )}
          <div className="grid justify-items-start gap-1.5">
            <Button size="small" onClick={() => fileInput.current?.click()}>{"image" in value ? m.icon_image_replace() : m.icon_image_upload()}</Button>
            <span role={imageError ? "alert" : undefined} className={cn("text-xs", imageError ? "text-accent-red" : "text-subtext")}>
              {imageError ? m.icon_image_error() : m.icon_image_hint()}
            </span>
          </div>
          <input
            ref={fileInput}
            type="file"
            accept="image/png,image/jpeg,image/webp,image/gif,image/svg+xml"
            hidden
            onChange={(event) => {
              const file = event.target.files?.[0];
              event.target.value = "";
              if (!file) return;
              setImageError(false);
              void encodeIconImage(file).then((image) => onChange({ image, color }), () => setImageError(true));
            }}
          />
        </div>
      )}
    </div>
  );
}

const EMOJI_CATEGORIES: [category: string, label: () => string, Icon: LucideIcon][] = [
  ["Smileys & Emotion", m.icon_emoji_smileys, Smile],
  ["People & Body", m.icon_emoji_people, User],
  ["Animals & Nature", m.icon_emoji_nature, Leaf],
  ["Food & Drink", m.icon_emoji_food, Apple],
  ["Travel & Places", m.icon_emoji_travel, Plane],
  ["Activities", m.icon_emoji_activities, Trophy],
  ["Objects", m.icon_emoji_objects, Lightbulb],
  ["Symbols", m.icon_emoji_symbols, Heart],
  ["Flags", m.icon_emoji_flags, Flag],
];

/** The emoji data is large, so it loads the first time the Emoji tab opens. */
const loadEmoji = () => import("gemoji").then((module) => module.gemoji);

function EmojiPicker({ value, cell, onPick }: { value: string | null; cell: string; onPick: (emoji: string) => void }) {
  const [emoji, setEmoji] = useState<Gemoji[] | null>(null);
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    void loadEmoji().then((loaded) => { if (live) setEmoji(loaded); });
    return () => { live = false; };
  }, []);

  const search = query.trim().toLowerCase();
  const shownCategory = category ?? emoji?.find((entry) => entry.emoji === value)?.category ?? EMOJI_CATEGORIES[0][0];
  const shown = emoji?.filter((entry) => search
    ? entry.names.some((name) => name.includes(search)) || entry.tags.some((tag) => tag.includes(search)) || entry.description.includes(search)
    : entry.category === shownCategory) ?? [];
  const categoryLabel = EMOJI_CATEGORIES.find(([name]) => name === shownCategory)?.[1]() ?? shownCategory;

  return (
    <div className="grid gap-2">
      <label className="relative block">
        <span className="sr-only">{m.icon_emoji_search()}</span>
        <Search size={14} className="pointer-events-none absolute start-2.5 top-1/2 -translate-y-1/2 text-muted" aria-hidden="true" />
        <Input type="search" autoComplete="off" spellCheck={false} placeholder={m.icon_emoji_search()} value={query} onChange={(event) => setQuery(event.target.value)} className="ps-8 [&::-webkit-search-cancel-button]:hidden" />
      </label>
      <div role="group" aria-label={m.icon_emoji_category()} className="flex justify-between">
        {EMOJI_CATEGORIES.map(([name, label, Icon]) => (
          <button
            key={name}
            type="button"
            aria-pressed={!search && name === shownCategory}
            aria-label={label()}
            className="grid size-7 place-items-center rounded-md text-subtext outline-none hover:bg-hover-muted hover:text-text focus-visible:outline-2 focus-visible:outline-focus aria-pressed:bg-hover-muted aria-pressed:text-text"
            onClick={() => { setQuery(""); setCategory(name); }}
          >
            <Icon size={15} aria-hidden="true" />
          </button>
        ))}
      </div>
      <div className="h-48 overflow-y-auto pe-1">
        {emoji === null || shown.length === 0 ? (
          <p role="status" className="m-0 py-8 text-center text-sm text-subtext">
            {emoji === null ? m.icon_emoji_loading() : m.icon_emoji_none({ query: query.trim() })}
          </p>
        ) : (
          <div role="radiogroup" aria-label={search ? m.icon_emoji_matches() : categoryLabel} className={GRID}>
            {shown.map((entry) => (
              <button key={entry.emoji} type="button" role="radio" aria-checked={entry.emoji === value} aria-label={entry.description} className={cell} onClick={() => onPick(entry.emoji)}>
                <span className="text-lg leading-none">{entry.emoji}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/** The tile with a pencil badge, opening the picker; each pick reaches `onChange` at once. */
export function AgentIconButton({ value, brand, onChange, disabled }: { value: AgentIcon; brand: Logo; onChange: (icon: AgentIcon) => void; disabled?: boolean }) {
  const { portalContainer } = useContext(WorkspaceDomContext);
  return (
    <Popover.Root>
      <Popover.Trigger
        type="button"
        disabled={disabled}
        aria-label={m.icon_change()}
        className="group relative inline-grid shrink-0 rounded-xl outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus disabled:cursor-default"
      >
        <AgentIconTile icon={value} brand={brand} size="lg" />
        {!disabled && (
          <span className="absolute -end-1 -bottom-1 grid size-4.5 place-items-center rounded-full border border-border bg-background text-subtext shadow-control-subtle group-hover:text-text">
            <Pencil size={10} aria-hidden="true" />
          </span>
        )}
      </Popover.Trigger>
      <Popover.Portal container={portalContainer}>
        {/* Above the z-200 dialogs, since the wizard opens it too. */}
        <Popover.Positioner className="z-210" side="bottom" align="start" sideOffset={6} collisionPadding={8}>
          <Popover.Popup aria-label={m.icon_change()} className="w-76 max-w-[calc(100vw_-_16px)] rounded-lg border border-border bg-background p-3 shadow-dropdown outline-none">
            <AgentIconPicker value={value} brand={brand} onChange={onChange} />
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** Image icons are stored inline, so they are cropped square and kept small. */
async function encodeIconImage(file: Blob): Promise<Extract<AgentIcon, { image: unknown }>["image"]> {
  const bitmap = await createImageBitmap(file);
  const side = Math.min(bitmap.width, bitmap.height);
  const canvas = document.createElement("canvas");
  const encode = (pixels: number) => {
    canvas.width = pixels;
    canvas.height = pixels;
    canvas.getContext("2d")!.drawImage(bitmap, (bitmap.width - side) / 2, (bitmap.height - side) / 2, side, side, 0, 0, pixels, pixels);
    // Browsers without a WebP encoder return PNG, which is also accepted.
    const [header, data] = canvas.toDataURL("image/webp", 0.9).split(",", 2);
    return { type: header.includes("image/webp") ? "image/webp" as const : "image/png" as const, data };
  };
  // The server takes 64 KiB; a PNG of a photo can need a smaller square to fit.
  let pixels = 128;
  let image = encode(pixels);
  while (image.data.length * 0.75 > 60_000 && pixels > 64) image = encode(pixels -= 32);
  bitmap.close();
  return image;
}
