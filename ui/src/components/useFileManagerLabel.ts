import { useQuery } from "@tanstack/react-query";
import { autoDir } from "../i18n";
import { useLocale } from "../locale";
import { m } from "../paraglide/messages.js";
import { getFileManagerQuery } from "../queries/settings";

export function useFileManagerLabel() {
  const locale = useLocale();
  const { data } = useQuery(getFileManagerQuery(locale));
  const name = data?.os === "macos" ? m.file_manager_finder()
    : data?.os === "windows" ? m.file_manager_explorer()
      : data?.name;
  return name ? m.reveal_in_named_file_manager({ manager: autoDir(name) }) : m.file_viewer_reveal_in_file_manager();
}
