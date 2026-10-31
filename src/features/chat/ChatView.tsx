import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, Notice, Select, Spinner, Textarea } from "../../components/ui";
import {
  Channel,
  commands,
  errorMessage,
  unwrap,
  type Approval,
  type Attachment,
  type Chat,
  type ChatEvent,
  type ChatSummary,
  type Message,
  type ToolView,
} from "../../lib/api";
import { useAppData } from "../../lib/data";
import { numberFormat } from "../../lib/format";
import { useSession } from "../../lib/session";
import { ReportViewer } from "../reports/ReportViewer";
import { Markdown } from "./Markdown";
import { ToolCard, type ReportLink } from "./ToolCard";

const DATA_FILES = ["csv", "tsv", "txt", "xlsx", "xlsm", "xlsb", "xls", "ods"];

/** What the turn in progress has shown so far, in order. */
type Live = (
  | { kind: "text"; text: string }
  | { kind: "tool"; id: string; name: string; input: unknown; view?: ToolView; waiting?: boolean }
)[];

export function ChatView() {
  const { t, i18n } = useTranslation();
  const { catalog } = useAppData();
  const engine = useSession((s) => s.setup?.engine);
  const [chats, setChats] = useState<ChatSummary[]>([]);
  const [chat, setChat] = useState<Chat | null>(null);
  const [draft, setDraft] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [question, setQuestion] = useState<string | null>(null);
  const [live, setLive] = useState<Live | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [viewing, setViewing] = useState<ReportLink | null>(null);
  const stopping = useRef(false);
  const bottom = useRef<HTMLDivElement>(null);
  const busy = live !== null;

  useEffect(() => {
    void commands.listChats().then(setChats);
  }, []);

  // Follow the answer as it streams in.
  useEffect(() => {
    bottom.current?.scrollIntoView({ block: "end" });
  }, [live, chat]);

  async function openChat(id: string | null) {
    setError(null);
    setAttachments([]);
    try {
      setChat(id ? await unwrap(commands.loadChat(id)) : null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function removeChat(id: string) {
    await unwrap(commands.deleteChat(id)).catch(() => {});
    if (chat?.id === id) setChat(null);
    setChats(await commands.listChats());
  }

  async function attach() {
    const path = await open({ multiple: false, directory: false, filters: [{ name: t("reports.fileTypes"), extensions: DATA_FILES }] });
    if (!path) return;
    try {
      const summary = await unwrap(commands.inspectSource(path, null));
      const sheet = summary.format.kind === "spreadsheet" ? summary.format.sheet : null;
      setAttachments((list) => [...list.filter((a) => a.path !== path), { name: summary.file, path, sheet }]);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  function onEvent(event: ChatEvent, chatId: { current: string | null }) {
    if (event.kind === "started") {
      chatId.current = event.id;
      return;
    }
    setLive((items) => {
      const list = [...(items ?? [])];
      const last = list[list.length - 1];
      switch (event.kind) {
        case "text":
          if (last?.kind === "text") list[list.length - 1] = { kind: "text", text: last.text + event.text };
          else list.push({ kind: "text", text: event.text });
          break;
        case "toolStarted":
          list.push({ kind: "tool", id: event.id, name: event.name, input: event.input });
          break;
        case "approvalNeeded":
        case "toolFinished":
          return list.map((item) =>
            item.kind === "tool" && item.id === event.id
              ? event.kind === "approvalNeeded"
                ? { ...item, waiting: true }
                : { ...item, waiting: false, view: event.view }
              : item,
          );
      }
      return list;
    });
  }

  async function send() {
    const text = draft.trim();
    if (!text || busy) return;
    const chatId = { current: chat?.id ?? null };
    const channel = new Channel<ChatEvent>();
    channel.onmessage = (event) => onEvent(event, chatId);
    stopping.current = false;
    setDraft("");
    setError(null);
    setQuestion(text);
    setLive([]);
    try {
      const updated = await unwrap(
        commands.sendMessage(
          chat?.id ?? null,
          text,
          attachments,
          {
            language: i18n.resolvedLanguage ?? "en",
            numberFormat: numberFormat(),
            focusNames: Object.fromEntries(catalog.focuses.map((f) => [f.id, t(`reports.focus.${f.id}.name`)])),
          },
          channel,
        ),
      );
      setChat(updated);
      setAttachments([]);
    } catch (e) {
      // The conversation was saved up to where it stopped; show that.
      if (!stopping.current) setError(errorMessage(e));
      if (chatId.current) {
        const id = chatId.current;
        await unwrap(commands.loadChat(id)).then(setChat, () => {});
        setAttachments([]);
      }
    } finally {
      setLive(null);
      setQuestion(null);
      setChats(await commands.listChats());
    }
  }

  function stop() {
    stopping.current = true;
    void commands.stopChat();
  }

  function answer(id: string, approval: Approval) {
    setLive((items) => items?.map((item) => (item.kind === "tool" && item.id === id ? { ...item, waiting: false } : item)) ?? null);
    void commands.answerApproval(id, approval);
  }

  function openFile(path: string, reveal = false) {
    void unwrap(commands.openFile(path, reveal)).catch((e: unknown) => setError(errorMessage(e)));
  }

  if (viewing) {
    return <ReportViewer report={viewing} onBack={() => setViewing(null)} onOpen={openFile} />;
  }

  const results = new Map<string, ToolView | undefined>();
  for (const m of chat?.messages ?? []) {
    for (const p of m.parts) if (p.type === "toolResult") results.set(p.id, p.view ?? undefined);
  }
  const date = new Intl.DateTimeFormat(i18n.resolvedLanguage, { dateStyle: "medium" });

  return (
    <div className="flex h-[calc(100vh-4rem)] gap-5">
      <aside className="hidden w-56 shrink-0 flex-col gap-2 xl:flex">
        <Button variant="secondary" disabled={busy} onClick={() => void openChat(null)}>
          {t("chat.new")}
        </Button>
        <ul className="min-h-0 flex-1 space-y-0.5 overflow-y-auto text-sm">
          {chats.map((c) => (
            <li key={c.id} className="group relative">
              <button
                type="button"
                disabled={busy}
                onClick={() => void openChat(c.id)}
                className={`block w-full rounded-lg px-2.5 py-1.5 text-left ${
                  c.id === chat?.id ? "bg-zinc-200/70 dark:bg-zinc-800" : "hover:bg-zinc-100 dark:hover:bg-zinc-800/60"
                }`}
              >
                <div className="truncate pr-5">{c.title}</div>
                <div className="text-xs text-zinc-500">{date.format(new Date(c.updated))}</div>
              </button>
              <button
                type="button"
                aria-label={t("chat.delete")}
                title={t("chat.delete")}
                disabled={busy}
                onClick={() => void removeChat(c.id)}
                className="absolute top-1.5 right-1.5 hidden rounded px-1 text-zinc-400 group-hover:block hover:text-red-600"
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      </aside>

      <section className="flex min-w-0 flex-1 flex-col">
        {/* Narrow windows: the conversation list folds into this bar. */}
        <div className="mb-3 flex gap-2 xl:hidden">
          <Select
            value={chat?.id ?? ""}
            disabled={busy}
            className="min-w-0 flex-1"
            onChange={(e) => void openChat(e.target.value || null)}
          >
            <option value="">{t("chat.new")}</option>
            {chats.map((c) => (
              <option key={c.id} value={c.id}>
                {c.title}
              </option>
            ))}
          </Select>
          {chat && (
            <Button variant="ghost" disabled={busy} onClick={() => void removeChat(chat.id)}>
              {t("chat.delete")}
            </Button>
          )}
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto pr-1">
          <div className="mx-auto max-w-3xl space-y-4 pb-4">
            {!chat && !busy && <Welcome hasEngine={engine !== undefined} />}
            {chat && chat.attachments.length > 0 && (
              <div className="text-xs text-zinc-500">
                {t("chat.attached")}: {chat.attachments.map((a) => a.name).join(", ")}
              </div>
            )}
            {chat?.messages.map((m, i) => (
              <MessageView
                key={i}
                message={m}
                results={results}
                onOpen={(p) => openFile(p)}
                onReport={setViewing}
              />
            ))}
            {question && <UserBubble text={question} />}
            {live && (
              <div>
                {live.length === 0 && (
                  <div className="flex items-center gap-2 text-sm text-zinc-500">
                    <Spinner /> {t("chat.thinking")}
                  </div>
                )}
                {live.map((item, i) =>
                  item.kind === "text" ? (
                    <Markdown key={i} text={item.text} />
                  ) : (
                    <ToolCard
                      key={item.id}
                      name={item.name}
                      input={item.input}
                      view={item.view}
                      waiting={item.waiting}
                      onAnswer={(a) => answer(item.id, a)}
                      onOpen={(p) => openFile(p)}
                      onReport={setViewing}
                    />
                  ),
                )}
              </div>
            )}
            {error && <Notice tone="bad">{error}</Notice>}
            <div ref={bottom} />
          </div>
        </div>

        <div className="mx-auto w-full max-w-3xl pt-2">
          {attachments.length > 0 && (
            <div className="mb-2 flex flex-wrap gap-1.5">
              {attachments.map((a) => (
                <span key={a.path} className="flex items-center gap-1 rounded-full bg-zinc-100 px-2.5 py-0.5 text-xs dark:bg-zinc-800">
                  {a.name}
                  {a.sheet && <span className="text-zinc-500">· {a.sheet}</span>}
                  <button
                    type="button"
                    aria-label={t("connections.remove")}
                    className="text-zinc-400 hover:text-red-600"
                    onClick={() => setAttachments((list) => list.filter((x) => x.path !== a.path))}
                  >
                    ×
                  </button>
                </span>
              ))}
            </div>
          )}
          <div className="flex items-end gap-2">
            <Textarea
              rows={2}
              value={draft}
              disabled={!engine}
              placeholder={t("chat.placeholder")}
              className="min-h-[3.25rem] flex-1"
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  void send();
                }
              }}
            />
            <div className="flex flex-col gap-1.5">
              <Button variant="ghost" className="px-3 py-1.5" disabled={busy || !engine} onClick={() => void attach()}>
                {t("chat.attach")}
              </Button>
              {busy ? (
                <Button variant="secondary" className="px-3 py-1.5" onClick={stop}>
                  {t("chat.stop")}
                </Button>
              ) : (
                <Button className="px-3 py-1.5" disabled={!draft.trim() || !engine} onClick={() => void send()}>
                  {t("chat.send")}
                </Button>
              )}
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}

function UserBubble({ text }: { text: string }) {
  return (
    <div className="flex justify-end">
      <div className="max-w-[85%] rounded-2xl rounded-br-md bg-orange-50 px-3.5 py-2 text-sm whitespace-pre-wrap dark:bg-orange-950/40">
        {text}
      </div>
    </div>
  );
}

function MessageView({
  message,
  results,
  onOpen,
  onReport,
}: {
  message: Message;
  results: Map<string, ToolView | undefined>;
  onOpen: (path: string) => void;
  onReport: (report: ReportLink) => void;
}) {
  if (message.role === "user") {
    const text = message.parts.map((p) => (p.type === "text" ? p.text : "")).join("");
    // Tool results travel as user messages; they are drawn on their tool's card.
    return text ? <UserBubble text={text} /> : null;
  }
  return (
    <div>
      {message.parts.map((p, i) =>
        p.type === "text" ? (
          <Markdown key={i} text={p.text} />
        ) : p.type === "toolCall" ? (
          <ToolCard
            key={p.id}
            name={p.name}
            input={p.input}
            view={results.get(p.id)}
            onAnswer={() => {}}
            onOpen={onOpen}
            onReport={onReport}
          />
        ) : null,
      )}
    </div>
  );
}

function Welcome({ hasEngine }: { hasEngine: boolean }) {
  const { t } = useTranslation();
  return (
    <div className="pt-10 text-center">
      <h1 className="text-2xl font-semibold tracking-tight">{t("chat.title")}</h1>
      <p className="mx-auto mt-2 max-w-md text-sm text-zinc-500">{t(hasEngine ? "chat.welcome" : "chat.noEngine")}</p>
    </div>
  );
}
