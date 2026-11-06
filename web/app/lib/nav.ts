import {
  BookImage,
  Clapperboard,
  ClockFading,
  Heart,
  House,
  Images,
  ListVideo,
  MapPinned,
  Search,
  Trash2,
  Users,
  type LucideIcon,
} from "lucide-react";

export interface NavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  /** Shown on the page until the feature ships. */
  blurb?: string;
  milestone?: string;
}

export const primaryNav: NavItem[] = [
  { to: "/", label: "Home", icon: House },
  { to: "/videos", label: "Videos", icon: Clapperboard },
  {
    to: "/photos",
    label: "Photos",
    icon: Images,
    blurb: "A timeline of your photos grouped by day, including HEIC, RAW and Live Photos.",
    milestone: "M3",
  },
  {
    to: "/map",
    label: "Map",
    icon: MapPinned,
    blurb: "Browse your photos and videos by where they were taken.",
    milestone: "M3",
  },
];

export const libraryNav: NavItem[] = [
  {
    to: "/history",
    label: "History",
    icon: ClockFading,
    blurb: "Everything you've watched, so you can pick up where you left off.",
    milestone: "M2",
  },
  {
    to: "/playlists",
    label: "Playlists",
    icon: ListVideo,
    blurb: "Ordered collections of videos that play one after another.",
    milestone: "M2",
  },
  {
    to: "/albums",
    label: "Albums",
    icon: BookImage,
    blurb: "Hand-picked collections of photos and videos.",
    milestone: "M3",
  },
  {
    to: "/liked",
    label: "Liked",
    icon: Heart,
    blurb: "The moments you've marked as favourites.",
    milestone: "M2",
  },
  { to: "/trash", label: "Trash", icon: Trash2 },
];

export const sharedNav: NavItem[] = [
  {
    to: "/shared",
    label: "Shared with me",
    icon: Users,
    blurb: "Albums, channels and videos that family and friends have shared with you.",
    milestone: "M4",
  },
];

const searchItem: NavItem = {
  to: "/search",
  label: "Search",
  icon: Search,
  blurb: "Search titles, descriptions and dates across your whole library.",
  milestone: "M2",
};

export const comingSoonPages = [...primaryNav, ...libraryNav, ...sharedNav, searchItem].filter(
  (item) => item.milestone,
);
