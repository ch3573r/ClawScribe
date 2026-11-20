"use client";

import { HardDrive } from "lucide-react";
import { Button } from "@/components/ui/button";
import { openStorageDialog } from "@/lib/storage-dialog-controller";

export function ArchiveStorageButton() {
  return <Button variant="outline" onClick={openStorageDialog}><HardDrive aria-hidden="true" />Storage</Button>;
}
