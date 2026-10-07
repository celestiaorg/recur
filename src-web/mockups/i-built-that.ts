import { mount } from "svelte";
import BuiltThat from "../src/mockups/BuiltThat.svelte";
import "../src/lib/file-icons/generated/fonts.css";
import "../src/lib/file-icons/generated/colors.css";
import "../src/lib/file-icons/generated/icons.css";
import "../src/app.css";

mount(BuiltThat, { target: document.getElementById("app")! });
