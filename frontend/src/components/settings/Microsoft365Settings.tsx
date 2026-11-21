"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, LogIn, LogOut } from "lucide-react";
import {
  Microsoft365Icon, OneDriveIcon, OneNoteIcon, PlannerIcon, ToDoIcon,
} from "@/components/IntegrationIcons";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger,
} from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SettingsCard, SettingsRow, SettingsStack, StatusText } from "@/components/ui/settings-layout";
import { CreateRow, IconAction, IconTile, NoteRow, ROW_SELECT, TWO_COLUMNS } from "@/components/settings/settings-parts";
import { useMicrosoftExport } from "@/hooks/useMicrosoftExport";
import { getExportDestinations, setExportDestinations } from "@/lib/exportDestinations";
import { ONENOTE_LARGE_LIBRARY_MESSAGE, microsoftExportService, type DriveDestination } from "@/services/microsoftExportService";

// OneNote notebook names reject ? * \ / : < > | ' # and must be <= 128 chars.
// Mirror the backend sanitizer so the field shows what will actually be created.
const NOTEBOOK_FORBIDDEN = /[?*\\/:<>|'#]/g;
export function sanitizeNotebookName(raw: string): string {
  return raw.replace(NOTEBOOK_FORBIDDEN, " ").replace(/\s+/g, " ").trimStart().slice(0, 128);
}

export function sanitizeToDoListName(raw: string): string {
  return raw.replace(/\s+/g, " ").trimStart().slice(0, 255);
}

export function sanitizeOneDriveFolderName(raw: string): string {
  return raw
    .replace(/["*:<>?/\\|]/g, " ")
    .replace(/\s+/g, " ")
    .trimStart()
    .slice(0, 120);
}

const NEW_OPTION = "__new__";
const EMPTY_OPTION = "__empty__";

type Microsoft = ReturnType<typeof useMicrosoftExport>;

/** Shown inside a card while the card's own connection check has not finished. */
function CheckingAccount() {
  return <NoteRow role="status">Checking your Microsoft account…</NoteRow>;
}

export function MicrosoftAccountCard({ ms }: { ms: Microsoft }) {
  const state = ms.connection.state;
  const connected = state === "connected";
  const connecting = state === "connecting" || ms.signingIn;
  const name = ms.connection.userDisplayName ?? ms.connection.userEmail ?? "Microsoft user";
  const unavailable = ms.connection.unavailableExports ?? [];
  const missing = ms.connection.missingScopes ?? [];

  let title = "Not signed in";
  let description = "Sign in to export to OneNote, OneDrive, Planner and To Do, and to look up calendar events";
  if (connected) {
    title = `Signed in as ${name}`;
    description = ms.connection.userEmail && ms.connection.userEmail !== name
      ? `${ms.connection.userEmail} · exports and calendar lookups use this account`
      : "Exports and calendar lookups use this account";
  } else if (connecting) {
    title = "Signing in";
    description = "Waiting for Microsoft sign-in to complete";
  } else if (state === "expired") {
    title = "Session expired";
    description = "Sign in again to re-enable exports";
  }

  return (
    <SettingsCard>
      <SettingsRow icon={<IconTile><Microsoft365Icon className="h-5 w-5" /></IconTile>} title={title} description={description}>
        {connected ? (
          <>
            <StatusText tone="ok">Connected</StatusText>
            <Button type="button" variant="outline" size="sm" onClick={ms.signOut}>
              <LogOut aria-hidden="true" />Sign out
            </Button>
          </>
        ) : connecting ? (
          <>
            <StatusText tone="active"><Loader2 aria-hidden="true" className="h-3.5 w-3.5 animate-spin" />Waiting for browser</StatusText>
            <Button type="button" variant="outline" size="sm" onClick={ms.cancelSignIn}>Cancel sign-in</Button>
          </>
        ) : (
          <>
            <StatusText tone={state === "expired" ? "warning" : "off"}>{state === "expired" ? "Expired" : "Signed out"}</StatusText>
            <Button type="button" onClick={ms.signIn}>
              <LogIn aria-hidden="true" />Sign in with Microsoft
            </Button>
          </>
        )}
      </SettingsRow>
      {connecting && <NoteRow role="status">Complete sign-in in your browser, then return to ClawScribe.</NoteRow>}
      {connected && unavailable.length > 0 && (
        <NoteRow tone="warning">Unavailable with this account&apos;s current permissions: {unavailable.join(", ")}.</NoteRow>
      )}
      {connected && ms.connection.sessionOnly && (
        <NoteRow>Sign-in will not persist after closing ClawScribe because offline access was not granted.</NoteRow>
      )}
      {connected && missing.length > 0 && (
        <SettingsRow
          title="Missing permissions"
          description={`${missing.join(", ")}. Your administrator may need to approve them.`}
        >
          <Button type="button" variant="outline" size="sm" disabled={ms.signingIn} onClick={ms.requestMissingPermissions}>
            Request missing permissions
          </Button>
        </SettingsRow>
      )}
      {ms.error && <NoteRow tone="error">{ms.error}</NoteRow>}
    </SettingsCard>
  );
}

export function OneNoteCard() {
  const ms = useMicrosoftExport();
  const saved = getExportDestinations();
  const [selectedNotebook, setSelectedNotebook] = useState<string>(saved.notebookId ?? "");
  const [creatingNotebook, setCreatingNotebook] = useState(false);
  const [newNotebookName, setNewNotebookName] = useState("");
  const [savingNotebook, setSavingNotebook] = useState(false);
  const newNotebookInputRef = useRef<HTMLInputElement>(null);

  const submitNewNotebook = async () => {
    const name = sanitizeNotebookName(newNotebookName).trim();
    if (!name) return;
    setSavingNotebook(true);
    const nb = await ms.createNotebook(name);
    setSavingNotebook(false);
    if (nb) {
      setSelectedNotebook(nb.id);
      setCreatingNotebook(false);
      setNewNotebookName("");
    }
  };

  const isConnected = ms.connection.state === "connected";

  const { loadNotebooks } = ms;
  useEffect(() => {
    if (isConnected) void loadNotebooks();
  }, [isConnected, loadNotebooks]);

  // Persist the notebook destination. Exports create a fresh dated section in
  // this notebook, so Settings never lists or stores section ids.
  useEffect(() => {
    if (!selectedNotebook) return;
    const notebookName = ms.notebooks.find((n) => n.id === selectedNotebook)?.displayName;
    setExportDestinations({
      notebookId: selectedNotebook,
      notebookName,
      sectionId: undefined,
      sectionName: undefined,
    });
  }, [selectedNotebook, ms.notebooks]);

  const selectedNotebookMissing = !!selectedNotebook && !ms.notebooks.some((nb) => nb.id === selectedNotebook);

  return (
    <SettingsCard
      icon={<OneNoteIcon className="h-5 w-5" />}
      title="OneNote"
      description="Each meeting gets a new dated section"
      actions={isConnected && <IconAction label="Reload notebooks" onClick={() => void ms.loadNotebooks()} loading={ms.loadingNotebooks} />}
    >
      {!isConnected ? <CheckingAccount /> : (
        <>
          <SettingsRow title="Notebook" htmlFor="onenote-notebook">
            <Select
              value={(creatingNotebook ? NEW_OPTION : selectedNotebook) || EMPTY_OPTION}
              onValueChange={(value) => {
                const nextValue = value === EMPTY_OPTION ? "" : value;
                if (nextValue === NEW_OPTION) {
                  setCreatingNotebook(true);
                } else {
                  setCreatingNotebook(false);
                  setSelectedNotebook(nextValue);
                }
              }}
              disabled={ms.loadingNotebooks}
            >
              <SelectTrigger id="onenote-notebook" aria-label="Notebook" className={ROW_SELECT}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent onCloseAutoFocus={(event) => {
                const input = newNotebookInputRef.current;
                if (input) {
                  event.preventDefault();
                  input.focus();
                }
              }}>
                <SelectItem value={EMPTY_OPTION}>{ms.loadingNotebooks ? "Loading…" : "Select a notebook"}</SelectItem>
                {ms.notebooks.map((nb) => (
                  <SelectItem key={nb.id} value={nb.id}>{nb.displayName}</SelectItem>
                ))}
                {selectedNotebookMissing && (
                  <SelectItem value={selectedNotebook}>{saved.notebookName ?? "Saved notebook"}</SelectItem>
                )}
                <SelectItem value={NEW_OPTION}>New notebook…</SelectItem>
              </SelectContent>
            </Select>
          </SettingsRow>
          {creatingNotebook && (
            <CreateRow
              label="New notebook name"
              value={newNotebookName}
              onChange={(value) => setNewNotebookName(sanitizeNotebookName(value))}
              onSubmit={() => void submitNewNotebook()}
              onCancel={() => setCreatingNotebook(false)}
              placeholder="Meeting notes"
              maxLength={128}
              saving={savingNotebook}
              inputRef={newNotebookInputRef}
              hint={`Can't contain ${"? * \\ / : < > | ' #"}. Those are removed automatically. Up to 128 characters.`}
            />
          )}
          {ms.oneNoteNotebookListingLimited && <NoteRow tone="warning">{ONENOTE_LARGE_LIBRARY_MESSAGE}</NoteRow>}
          {ms.error && <NoteRow tone="error">{ms.error}</NoteRow>}
          {!ms.loadingNotebooks && !ms.error && ms.notebooks.length === 0 && (
            <NoteRow>
              No OneNote notebooks were returned for this account. Check that you signed in with the account that owns them
              and that ClawScribe has the OneNote permission, then reload.
            </NoteRow>
          )}
        </>
      )}
    </SettingsCard>
  );
}

export function ToDoCard() {
  const ms = useMicrosoftExport();
  const saved = getExportDestinations();
  const [selectedList, setSelectedList] = useState<string>(saved.todoListId ?? "");
  const [creatingList, setCreatingList] = useState(false);
  const [newListName, setNewListName] = useState("");
  const [savingList, setSavingList] = useState(false);
  const newListInputRef = useRef<HTMLInputElement>(null);
  const savingListRef = useRef(false);
  const isConnected = ms.connection.state === "connected";

  const submitNewList = async () => {
    if (savingListRef.current) return;
    const name = sanitizeToDoListName(newListName).trim();
    if (!name) return;
    savingListRef.current = true;
    setSavingList(true);
    try {
      const list = await ms.createToDoList(name);
      if (list) {
        setSelectedList(list.id);
        setCreatingList(false);
        setNewListName("");
      }
    } finally {
      savingListRef.current = false;
      setSavingList(false);
    }
  };

  const { loadToDoLists } = ms;
  useEffect(() => {
    if (isConnected) void loadToDoLists();
  }, [isConnected, loadToDoLists]);

  useEffect(() => {
    if (!selectedList) return;
    setExportDestinations({
      todoListId: selectedList,
      todoListName: ms.todoLists.find((l) => l.id === selectedList)?.displayName,
    });
  }, [selectedList, ms.todoLists]);

  const selectedListMissing = !!selectedList && !ms.todoLists.some((l) => l.id === selectedList);

  return (
    <SettingsCard
      icon={<ToDoIcon className="h-5 w-5" />}
      title="To Do"
      description="Reviewed action items become tasks"
      actions={isConnected && <IconAction label="Reload lists" onClick={() => void ms.loadToDoLists()} loading={ms.loadingToDoLists} />}
    >
      {!isConnected ? <CheckingAccount /> : (
        <>
          <SettingsRow title="List" htmlFor="todo-list">
            <Select
              value={(creatingList ? NEW_OPTION : selectedList) || EMPTY_OPTION}
              onValueChange={(value) => {
                const nextValue = value === EMPTY_OPTION ? "" : value;
                if (nextValue === NEW_OPTION) {
                  setCreatingList(true);
                } else {
                  setCreatingList(false);
                  setSelectedList(nextValue);
                }
              }}
              disabled={ms.loadingToDoLists}
            >
              <SelectTrigger id="todo-list" aria-label="To Do list" className={ROW_SELECT}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent onCloseAutoFocus={(event) => {
                const input = newListInputRef.current;
                if (input) {
                  event.preventDefault();
                  input.focus();
                }
              }}>
                <SelectItem value={EMPTY_OPTION}>{ms.loadingToDoLists ? "Loading…" : "Select a To Do list"}</SelectItem>
                {ms.todoLists.map((list) => (
                  <SelectItem key={list.id} value={list.id}>{list.displayName}</SelectItem>
                ))}
                {selectedListMissing && (
                  <SelectItem value={selectedList}>{saved.todoListName ?? "Saved To Do list"}</SelectItem>
                )}
                <SelectItem value={NEW_OPTION}>New To Do list…</SelectItem>
              </SelectContent>
            </Select>
          </SettingsRow>
          {creatingList && (
            <CreateRow
              label="New To Do list name"
              value={newListName}
              onChange={(value) => setNewListName(sanitizeToDoListName(value))}
              onSubmit={() => void submitNewList()}
              onCancel={() => setCreatingList(false)}
              placeholder="Meeting action items"
              maxLength={255}
              saving={savingList}
              inputRef={newListInputRef}
              hint="Creates a personal To Do list in your account and saves it as the destination"
            />
          )}
          {ms.error && <NoteRow tone="error">{ms.error}</NoteRow>}
          {!ms.loadingToDoLists && !ms.error && ms.todoLists.length === 0 && (
            <NoteRow>No To Do lists were returned for this account. Create one here, or reload after creating one elsewhere.</NoteRow>
          )}
        </>
      )}
    </SettingsCard>
  );
}

export function OneDriveCard() {
  const ms = useMicrosoftExport();
  const saved = getExportDestinations();
  const [rootDestination, setRootDestination] = useState<DriveDestination | null>(null);
  const [selectedDestination, setSelectedDestination] = useState<DriveDestination | null>(
    saved.oneDriveDestination ?? null,
  );
  const [loadingRoot, setLoadingRoot] = useState(false);
  const [sharingUrl, setSharingUrl] = useState("");
  const [resolvingUrl, setResolvingUrl] = useState(false);
  const [newFolderName, setNewFolderName] = useState("");
  const [creatingFolder, setCreatingFolder] = useState(false);
  const [includePdf, setIncludePdf] = useState(saved.oneDriveIncludePdf ?? true);
  const [createOrgLink, setCreateOrgLink] = useState(saved.oneDriveCreateOrganizationLink ?? false);
  const [localError, setLocalError] = useState<string | null>(null);
  const [folderDialogOpen, setFolderDialogOpen] = useState(false);
  const isConnected = ms.connection.state === "connected";
  const activeDestination = selectedDestination ?? rootDestination;

  const persistDestination = useCallback((destination: DriveDestination) => {
    setSelectedDestination(destination);
    setExportDestinations({ oneDriveDestination: destination });
  }, []);

  const loadRoot = useCallback(async () => {
    if (!isConnected) return null;
    setLoadingRoot(true);
    setLocalError(null);
    try {
      const destinations = await microsoftExportService.listOneDriveDestinations();
      const root = destinations[0] ?? null;
      setRootDestination(root);
      return root;
    } catch (e) {
      setLocalError(e instanceof Error ? e.message : String(e));
      return null;
    } finally {
      setLoadingRoot(false);
    }
  }, [isConnected]);

  useEffect(() => {
    if (isConnected) void loadRoot();
  }, [isConnected, loadRoot]);

  useEffect(() => {
    setExportDestinations({
      oneDriveIncludePdf: includePdf,
      oneDriveCreateOrganizationLink: createOrgLink,
    });
  }, [createOrgLink, includePdf]);

  const useRoot = async () => {
    const root = rootDestination ?? (await loadRoot());
    if (root) persistDestination(root);
  };

  const resolveSharingUrl = async () => {
    const url = sharingUrl.trim();
    if (!url) return;
    setResolvingUrl(true);
    setLocalError(null);
    try {
      const destination = await microsoftExportService.resolveOneDriveDestinationUrl(url);
      persistDestination(destination);
      setSharingUrl("");
    } catch (e) {
      setLocalError(e instanceof Error ? e.message : String(e));
    } finally {
      setResolvingUrl(false);
    }
  };

  const createFolder = async () => {
    const name = sanitizeOneDriveFolderName(newFolderName).trim();
    if (!name) return;
    const parent = activeDestination ?? (await loadRoot());
    if (!parent) return;

    setCreatingFolder(true);
    setLocalError(null);
    try {
      const destination = await microsoftExportService.createOneDriveDestinationFolder(parent, name);
      persistDestination(destination);
      setNewFolderName("");
    } catch (e) {
      setLocalError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreatingFolder(false);
    }
  };

  const busy = loadingRoot || resolvingUrl || creatingFolder;
  const destinationName = activeDestination?.name ?? (loadingRoot ? "Loading destination…" : "No folder selected");

  return (
    <SettingsCard
      icon={<OneDriveIcon className="h-5 w-5" />}
      title="OneDrive"
      description="Full meeting notes as DOCX, in OneDrive or SharePoint"
    >
      {!isConnected ? <CheckingAccount /> : (
        <>
          <Dialog open={folderDialogOpen} onOpenChange={setFolderDialogOpen}>
            <SettingsRow
              title="Destination folder"
              description={<span className="break-words font-mono text-xs">{destinationName}</span>}
            >
              <DialogTrigger asChild><Button type="button" size="sm" variant="outline">Change folder</Button></DialogTrigger>
            </SettingsRow>
            <DialogContent className="max-h-[85vh] overflow-y-auto">
              <DialogHeader>
                <DialogTitle>Change destination folder</DialogTitle>
                <DialogDescription>Choose a OneDrive or SharePoint folder. Changes are saved when you choose or create a folder.</DialogDescription>
              </DialogHeader>
              <div className="space-y-4">
                <div className="space-y-1">
                  <p className="text-sm font-medium">Current destination</p>
                  <p className="break-words text-sm">{destinationName}</p>
                  {activeDestination?.webUrl && (
                    <p className="break-all text-sm text-muted-foreground">{activeDestination.webUrl}</p>
                  )}
                </div>
                <Button type="button" size="sm" variant="outline" onClick={useRoot} disabled={busy}>
                  {loadingRoot && <Loader2 aria-hidden="true" className="animate-spin" />}Use the top-level folder
                </Button>
                <div className="space-y-2">
                  <label htmlFor="onedrive-folder-link" className="text-sm font-medium">Folder link</label>
                  <Input
                    id="onedrive-folder-link"
                    type="url"
                    value={sharingUrl}
                    onChange={event => setSharingUrl(event.target.value)}
                    onKeyDown={event => {
                      if (event.key === "Enter" && !busy) void resolveSharingUrl();
                    }}
                    placeholder="Paste OneDrive or SharePoint folder link"
                    disabled={busy}
                  />
                  <Button type="button" variant="outline" onClick={resolveSharingUrl} disabled={busy || !sharingUrl.trim()}>
                    {resolvingUrl && <Loader2 aria-hidden="true" className="animate-spin" />}Resolve folder
                  </Button>
                </div>
                <div className="space-y-2">
                  <label htmlFor="onedrive-subfolder-name" className="text-sm font-medium">New subfolder</label>
                  <Input
                    id="onedrive-subfolder-name"
                    type="text"
                    value={newFolderName}
                    onChange={event => setNewFolderName(sanitizeOneDriveFolderName(event.target.value))}
                    onKeyDown={event => {
                      if (event.key === "Enter" && !busy) void createFolder();
                    }}
                    placeholder="Create subfolder, e.g. ClawScribe"
                    maxLength={120}
                    disabled={busy}
                  />
                  <Button type="button" variant="outline" onClick={createFolder} disabled={busy || !newFolderName.trim()}>
                    {creatingFolder && <Loader2 aria-hidden="true" className="animate-spin" />}Create folder
                  </Button>
                </div>
                {(localError || ms.error) && <p role="alert" className="text-sm text-error-foreground">{localError ?? ms.error}</p>}
              </div>
              <DialogFooter><Button type="button" variant="outline" onClick={() => setFolderDialogOpen(false)}>Done</Button></DialogFooter>
            </DialogContent>
          </Dialog>
          <SettingsRow title="Include a PDF copy" htmlFor="onedrive-pdf">
            <Switch id="onedrive-pdf" checked={includePdf} onCheckedChange={() => setIncludePdf((value) => !value)} />
          </SettingsRow>
          <SettingsRow title="Organization-only links" htmlFor="onedrive-org-links" description="Links open only for people in your organization">
            <Switch id="onedrive-org-links" checked={createOrgLink} onCheckedChange={() => setCreateOrgLink((value) => !value)} />
          </SettingsRow>
          {!folderDialogOpen && (localError || ms.error) && <NoteRow tone="error">{localError ?? ms.error}</NoteRow>}
        </>
      )}
    </SettingsCard>
  );
}

export function PlannerCard() {
  const ms = useMicrosoftExport();
  const saved = getExportDestinations();
  const [selectedPlan, setSelectedPlan] = useState<string>(saved.planId ?? "");
  const [selectedBucket, setSelectedBucket] = useState<string>(saved.bucketId ?? "");
  const [creatingBucket, setCreatingBucket] = useState(false);
  const [newBucketName, setNewBucketName] = useState("");
  const [savingBucket, setSavingBucket] = useState(false);
  const newBucketInputRef = useRef<HTMLInputElement>(null);
  const [aiPolish, setAiPolish] = useState<boolean>(saved.plannerAiPolish ?? false);

  const toggleAiPolish = () => {
    const next = !aiPolish;
    setAiPolish(next);
    setExportDestinations({ plannerAiPolish: next });
  };

  const submitNewBucket = async () => {
    const name = newBucketName.replace(/\s+/g, " ").trim();
    if (!name || !selectedPlan) return;
    setSavingBucket(true);
    const bucket = await ms.createBucket(selectedPlan, name);
    setSavingBucket(false);
    if (bucket) {
      setSelectedBucket(bucket.id);
      setCreatingBucket(false);
      setNewBucketName("");
    }
  };

  const isConnected = ms.connection.state === "connected";

  const { loadPlans } = ms;
  useEffect(() => {
    if (isConnected) void loadPlans();
  }, [isConnected, loadPlans]);

  const { loadBuckets } = ms;
  useEffect(() => {
    if (selectedPlan) {
      void loadBuckets(selectedPlan);
    }
  }, [selectedPlan, loadBuckets]);

  useEffect(() => {
    if (!selectedBucket) return;
    setExportDestinations({
      planId: selectedPlan,
      planName: ms.plans.find((p) => p.id === selectedPlan)?.title,
      bucketId: selectedBucket,
      bucketName: ms.buckets.find((b) => b.id === selectedBucket)?.name,
    });
  }, [selectedBucket, selectedPlan, ms.plans, ms.buckets]);

  return (
    <SettingsCard
      icon={<PlannerIcon className="h-5 w-5" />}
      title="Planner"
      description="You review and route each task before it is created"
      actions={isConnected && <IconAction label="Reload plans" onClick={() => void ms.loadPlans()} loading={ms.loadingPlans} />}
    >
      {!isConnected ? <CheckingAccount /> : (
        <>
          <SettingsRow title="Plan" htmlFor="planner-plan">
            <Select
              value={ms.plans.some((plan) => plan.id === selectedPlan) ? selectedPlan : EMPTY_OPTION}
              onValueChange={(value) => {
                const nextValue = value === EMPTY_OPTION ? "" : value;
                setSelectedPlan(nextValue);
                setSelectedBucket("");
              }}
              disabled={ms.loadingPlans}
            >
              <SelectTrigger id="planner-plan" aria-label="Plan" className={ROW_SELECT}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={EMPTY_OPTION}>{ms.loadingPlans ? "Loading…" : "Select a plan"}</SelectItem>
                {ms.plans.map((p) => (
                  <SelectItem key={p.id} value={p.id}>{p.title}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </SettingsRow>
          <SettingsRow title="Default bucket" htmlFor="planner-bucket">
            <Select
              value={creatingBucket
                ? selectedPlan ? NEW_OPTION : EMPTY_OPTION
                : ms.buckets.some((bucket) => bucket.id === selectedBucket) ? selectedBucket : EMPTY_OPTION}
              onValueChange={(value) => {
                const nextValue = value === EMPTY_OPTION ? "" : value;
                if (nextValue === NEW_OPTION) {
                  setCreatingBucket(true);
                } else {
                  setCreatingBucket(false);
                  setSelectedBucket(nextValue);
                }
              }}
              disabled={!selectedPlan || ms.loadingBuckets}
            >
              <SelectTrigger id="planner-bucket" aria-label="Default bucket" className={ROW_SELECT}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent onCloseAutoFocus={(event) => {
                const input = newBucketInputRef.current;
                if (input) {
                  event.preventDefault();
                  input.focus();
                }
              }}>
                <SelectItem value={EMPTY_OPTION}>
                  {ms.loadingBuckets ? "Loading…" : !selectedPlan ? "Select a plan first" : "Select a bucket"}
                </SelectItem>
                {ms.buckets.map((b) => (
                  <SelectItem key={b.id} value={b.id}>{b.name}</SelectItem>
                ))}
                {selectedPlan && <SelectItem value={NEW_OPTION}>New bucket…</SelectItem>}
              </SelectContent>
            </Select>
          </SettingsRow>
          {creatingBucket && (
            <CreateRow
              label="New bucket name"
              value={newBucketName}
              onChange={(value) => setNewBucketName(value.slice(0, 255))}
              onSubmit={() => void submitNewBucket()}
              onCancel={() => setCreatingBucket(false)}
              placeholder="Action items"
              maxLength={255}
              saving={savingBucket}
              inputRef={newBucketInputRef}
            />
          )}
          <SettingsRow
            title="Offer AI polishing in Planner review"
            description="Choose Polish selected with AI in the export preview to send the selected tasks to your summary provider."
            htmlFor="planner-ai-polish"
          >
            <Switch id="planner-ai-polish" checked={aiPolish} onCheckedChange={toggleAiPolish} />
          </SettingsRow>
          {ms.error && <NoteRow tone="error">{ms.error}</NoteRow>}
          {!ms.loadingPlans && !ms.error && ms.plans.length === 0 && (
            <NoteRow>
              No Planner plans found for this account. Create a plan in Planner or Microsoft Teams, then reload. Plans
              can&apos;t be created from ClawScribe because they must belong to a Microsoft 365 group.
            </NoteRow>
          )}
        </>
      )}
    </SettingsCard>
  );
}

export function Microsoft365Settings() {
  const ms = useMicrosoftExport();
  const connected = ms.connection.state === "connected";
  return (
    <SettingsStack wide>
      <MicrosoftAccountCard ms={ms} />
      {connected && (
        <div className={TWO_COLUMNS}>
          <OneNoteCard />
          <ToDoCard />
          <OneDriveCard />
          <PlannerCard />
        </div>
      )}
    </SettingsStack>
  );
}
