"use client";

import { TaskExportPreview, type TaskExportPreviewProps } from "./PlannerExportPreview";

type ToDoExportPreviewProps = TaskExportPreviewProps & {
  listId: string;
  listName?: string;
};

export function ToDoExportPreview(props: ToDoExportPreviewProps) {
  return <TaskExportPreview {...props} destination={{ kind: "todo", id: props.listId, name: props.listName }} />;
}
