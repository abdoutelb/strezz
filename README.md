# stressTest
stress test for any website

TO RUN the solution replace `URL` with your site
*)- use `URL=URL node index.js`

*) `docker build -t stress-web --build-arg SITE_URL=URL .`

*) `docker run -itd -p 4000:4000 -label stress-web:latest` 

check your `localhost:4000`
