#![no_std]
use aidoku::{
	Chapter, DeepLinkResult, FilterValue, HomeComponent, HomeComponentValue, HomeLayout, Listing,
	ListingKind, MangaPageResult, MangaWithChapter, Result, Source, Viewer,
	alloc::{String, Vec},
	imports::{
		html::{Document, Element},
		net::Request,
	},
	prelude::*,
};
use madara::{Impl, Madara, Params, helpers};

const BASE_URL: &str = "https://mangadistrict.com";

// listing ids are the site's m_orderby values
const LISTINGS: [(&str, &str); 4] = [
	("latest", "Latest Updates"),
	("trending", "Trending"),
	("views", "Popular"),
	("new-manga", "New Releases"),
];

struct MangaDistrict;

fn listing_request(params: &Params, id: &str, page: i32) -> Result<Request> {
	Ok(Request::get(format!(
		"{BASE_URL}/{}?s=&post_type=wp-manga&m_orderby={id}",
		(params.search_page)(page)
	))?)
}

impl MangaDistrict {
	// the site doesn't support load more requests or the default next page selectors
	fn parse_manga_page(&self, params: &Params, html: &Document) -> MangaPageResult {
		MangaPageResult {
			entries: html
				.select(&params.search_manga_selector)
				.map(|els| {
					els.filter_map(|el| self.parse_manga_element(params, el))
						.collect()
				})
				.unwrap_or_default(),
			has_next_page: html.select_first(".wp-pagenavi .larger").is_some(),
		}
	}
}

impl Impl for MangaDistrict {
	fn new() -> Self {
		Self
	}

	fn params(&self) -> Params {
		Params {
			base_url: BASE_URL.into(),
			source_path: "series".into(),
			default_viewer: Viewer::Webtoon,
			datetime_format: "MMMM d, yyyy".into(),
			search_manga_selector: "div.page-listing-item".into(),
			// the first paragraph repeats the title
			details_description_selector: "div.summary__content > p:nth-child(2)".into(),
			// the first page break is a placeholder image
			page_list_selector: "div.page-break:not(:has(#image-99999))".into(),
			..Default::default()
		}
	}

	fn get_search_manga_list(
		&self,
		params: &Params,
		query: Option<String>,
		page: i32,
		filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let request = helpers::get_search_request(params, query, page, filters)?;
		let html = self.modify_request(params, request)?.html()?;
		Ok(self.parse_manga_page(params, &html))
	}

	fn get_manga_list(
		&self,
		params: &Params,
		listing: Listing,
		page: i32,
	) -> Result<MangaPageResult> {
		let request = listing_request(params, &listing.id, page)?;
		let html = self.modify_request(params, request)?.html()?;
		Ok(self.parse_manga_page(params, &html))
	}

	fn get_home(&self, params: &Params) -> Result<HomeLayout> {
		let requests = LISTINGS
			.iter()
			.map(|(id, _)| self.modify_request(params, listing_request(params, id, 1)?))
			.collect::<Result<Vec<_>>>()?;

		let components = Request::send_all(requests)
			.into_iter()
			.zip(LISTINGS)
			.filter_map(|(response, (id, name))| {
				let html = response.ok()?.get_html().ok()?;
				let listing = Some(Listing {
					id: id.into(),
					name: name.into(),
					kind: ListingKind::Default,
				});
				let value = if id == "latest" {
					HomeComponentValue::MangaChapterList {
						page_size: Some(4),
						entries: html
							.select(&params.search_manga_selector)?
							.filter_map(|el| {
								let chapter = self.parse_chapter_element(
									params,
									el.select_first(".chapter-item")?,
								)?;
								Some(MangaWithChapter {
									manga: self.parse_manga_element(params, el)?,
									chapter,
								})
							})
							.collect(),
						listing,
					}
				} else {
					HomeComponentValue::Scroller {
						entries: self
							.parse_manga_page(params, &html)
							.entries
							.into_iter()
							.map(Into::into)
							.collect(),
						listing,
					}
				};
				Some(HomeComponent {
					title: Some(name.into()),
					subtitle: None,
					value,
				})
			})
			.collect();

		Ok(HomeLayout { components })
	}

	fn parse_chapter_element(&self, params: &Params, element: Element) -> Option<Chapter> {
		let url_element = element.select_first("a")?;
		let url = url_element.attr("abs:href")?;
		let title_text = url_element.text()?;
		let chapter_number = helpers::find_first_f32(&title_text);
		// "Chapter 1 - Title"
		let title = title_text
			.split_once(" - ")
			.map(|(_, title)| title.trim().into())
			.or_else(|| chapter_number.is_none().then_some(title_text));
		// new chapters have a relative date in the link title instead of a date
		let date = element
			.select_first(".timediff i")
			.and_then(|el| el.text())
			.or_else(|| {
				element
					.select_first(".timediff a")
					.and_then(|el| el.attr("title"))
			});

		Some(Chapter {
			key: url.strip_prefix(BASE_URL).unwrap_or(&url).into(),
			title,
			chapter_number,
			date_uploaded: date.map(|date| helpers::parse_chapter_date(params, &date)),
			url: Some(url),
			..Default::default()
		})
	}

	fn handle_deep_link(&self, _params: &Params, url: String) -> Result<Option<DeepLinkResult>> {
		let Some(path) = url.strip_prefix(BASE_URL) else {
			return Ok(None);
		};
		let path = path
			.split(['?', '#'])
			.next()
			.unwrap_or_default()
			.trim_matches('/');
		let mut segments = path.split('/');

		// https://mangadistrict.com/series/<slug>/<chapter>/ (older links use /title/)
		let (Some("series" | "title"), Some(slug)) = (segments.next(), segments.next()) else {
			return Ok(None);
		};
		let manga_key = format!("/series/{slug}/");

		Ok(Some(match segments.next() {
			Some(chapter) => DeepLinkResult::Chapter {
				key: format!("{manga_key}{chapter}/"),
				manga_key,
			},
			None => DeepLinkResult::Manga { key: manga_key },
		}))
	}

	// series urls moved from /title/ to /series/
	fn handle_id_migration(&self, _params: &Params, id: String) -> Result<String> {
		Ok(if id.starts_with("/title/") {
			id.replacen("/title/", "/series/", 1)
		} else {
			id
		})
	}
}

register_source!(
	Madara<MangaDistrict>,
	Home,
	ListingProvider,
	DeepLinkHandler,
	MigrationHandler
);
